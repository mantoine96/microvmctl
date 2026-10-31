use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use anyhow::{Context, Result, bail};

use crate::ch::{Client, VmInfo};
use crate::vm;

#[derive(clap::Args)]
pub struct Args {
    /// VM name, optionally as `user@vm`.
    target: String,
    /// Remote command, plus any ssh options placed after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

pub fn run(state_dir: &Path, args: Args) -> Result<()> {
    let (user, name) = match args.target.split_once('@') {
        Some((user, name)) => (Some(user), name),
        None => (None, args.target.as_str()),
    };
    let vm = vm::find(state_dir, name)?;
    let info: VmInfo = Client::for_vm(&vm, "ssh")?.get("vm.info")?;
    let Some(vsock) = info.config.vsock else {
        bail!(
            "{name} has no vsock device, so there is no network-independent way to reach its sshd.\n\n\
             Add this to the VM's declaration (the CID must be unique among VMs on this host,\n\
             and at least 3) and restart it:\n\n    \
             microvm.vsock.cid = 3;\n    \
             microvm.vsock.ssh.enable = true;\n\n\
             The guest also needs a user you can log in as, e.g.:\n\n    \
             users.users.root.openssh.authorizedKeys.keys = [ \"ssh-ed25519 ...\" ];",
            name = vm.name,
        );
    };
    let socket = vm.dir.join(&vsock.socket);
    let exe = std::env::current_exe().context("locating the microvmctl binary")?;

    let mut ssh = Command::new("ssh");
    ssh.arg("-o").arg(format!(
        "ProxyCommand={} vsock-proxy {} 22",
        shell_quote(&exe),
        shell_quote(&socket)
    ));
    // The vsock socket is only reachable by root on this host, which already
    // authenticates the guest; and guests without persistent /etc get fresh
    // host keys on every boot, which would trip known_hosts checks.
    ssh.args([
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "LogLevel=ERROR",
    ]);
    if let Some(user) = user {
        ssh.args(["-l", user]);
    }
    ssh.arg(&vm.name).args(&args.command);
    Err(ssh.exec()).context("executing ssh")
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[derive(clap::Args)]
pub struct ProxyArgs {
    /// cloud-hypervisor's host-side vsock socket.
    socket: PathBuf,
    /// Guest vsock port.
    port: u32,
}

/// Bridges stdin/stdout to a guest vsock port, for use as an ssh
/// ProxyCommand. Speaks cloud-hypervisor's (Firecracker-compatible)
/// multiplexer handshake: send `CONNECT <port>\n`, expect `OK <port>\n`.
pub fn proxy(args: ProxyArgs) -> Result<()> {
    let stream = UnixStream::connect(&args.socket)
        .with_context(|| format!("connecting to {}", args.socket.display()))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    handshake(&mut reader, &mut &stream, args.port)?;

    let mut to_guest = stream.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut to_guest);
        let _ = to_guest.shutdown(Shutdown::Write);
    });
    // `reader` may already hold bytes the guest sent right after `OK`.
    copy_flushing(&mut reader, &mut io::stdout().lock())?;
    Ok(())
}

fn handshake(reader: &mut impl BufRead, writer: &mut impl Write, port: u32) -> Result<()> {
    writer.write_all(format!("CONNECT {port}\n").as_bytes())?;
    let mut reply = String::new();
    reader.read_line(&mut reply)?;
    if !reply.starts_with("OK ") {
        bail!(
            "guest refused vsock port {port} ({:?}); is sshd listening on vsock in the guest?",
            reply.trim()
        );
    }
    Ok(())
}

fn copy_flushing(from: &mut impl Read, to: &mut impl Write) -> io::Result<()> {
    let mut buf = [0; 16 * 1024];
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        to.write_all(&buf[..n])?;
        to.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_for_proxy_command() {
        assert_eq!(
            shell_quote(Path::new("/var/lib/microvms/a b")),
            "'/var/lib/microvms/a b'"
        );
        assert_eq!(shell_quote(Path::new("it's")), r"'it'\''s'");
    }

    #[test]
    fn checks_vsock_handshake() {
        let mut sent = Vec::new();
        handshake(&mut &b"OK 1073741824\n"[..], &mut sent, 22).unwrap();
        assert_eq!(sent, b"CONNECT 22\n");

        let err = handshake(&mut &b"connection refused\n"[..], &mut Vec::new(), 22)
            .unwrap_err()
            .to_string();
        assert!(err.contains("refused vsock port 22"), "{err}");
    }
}
