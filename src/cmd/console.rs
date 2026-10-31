use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::thread;

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::{FlockOperation, flock};
use rustix::termios::{OptionalActions, QueueSelector, Termios, tcflush, tcgetattr, tcsetattr};

use crate::ch::{Client, VmInfo};
use crate::vm::Vm;

/// Ctrl-], as in telnet and virsh.
const DETACH: u8 = 0x1d;

pub fn run(vm: &Vm) -> Result<()> {
    let info: VmInfo = Client::for_vm(vm, "console")?.get("vm.info")?;
    let console = &info.config.console;
    if console.mode != "Pty" {
        bail!(
            "{name} has no interactive console (its virtio-console mode is {mode:?}).\n\n\
             microvm.nix disables it by default. Add this to the VM's declaration and restart it:\n\n    \
             microvm.cloud-hypervisor.extraArgs = [ \"--console\" \"pty\" ];\n\n\
             The guest then runs a login prompt on /dev/hvc0, while kernel and boot output\n\
             keep going to the serial port and `microvmctl logs`.",
            name = vm.name,
            mode = console.mode,
        );
    }
    let path = console
        .file
        .as_ref()
        .context("cloud-hypervisor did not report the console's pty path")?;

    let pty = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    // Two readers on one pty would each get a random half of the output.
    flock(&pty, FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| anyhow!("the console of {} is already attached elsewhere", vm.name))?;
    // The pty's line discipline would otherwise echo and line-buffer
    // input on the host side, on top of the guest's own handling.
    make_raw(&pty)?;
    // Output buffered while nobody was attached can include terminal
    // queries (e.g. cursor position from the login script); replaying them
    // makes the local terminal answer into the guest shell as garbage.
    tcflush(&pty, QueueSelector::IFlush)?;

    let _terminal = RawTerminal::enter()?;
    eprint!(
        "Connected to {} console ({}). Press Ctrl-] to detach, Enter for a prompt.\r\n",
        vm.name,
        path.display()
    );

    let mut from_guest = pty.try_clone()?;
    thread::spawn(move || {
        let mut stdout = io::stdout();
        let mut buf = [0; 4096];
        while let Ok(n @ 1..) = from_guest.read(&mut buf) {
            if stdout
                .write_all(&buf[..n])
                .and_then(|_| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    });

    forward_input(io::stdin().lock(), &pty)?;
    eprint!("\r\nDetached from {}.\r\n", vm.name);
    Ok(())
}

fn forward_input(mut input: impl Read, mut pty: &File) -> Result<()> {
    let mut buf = [0; 1024];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        match buf[..n].iter().position(|&b| b == DETACH) {
            Some(pos) => return Ok(pty.write_all(&buf[..pos])?),
            None => pty.write_all(&buf[..n])?,
        }
    }
}

fn make_raw(fd: impl AsFd) -> Result<Termios> {
    let original = tcgetattr(&fd)?;
    let mut raw = original.clone();
    raw.make_raw();
    tcsetattr(&fd, OptionalActions::Now, &raw)?;
    Ok(original)
}

/// Puts the local terminal in raw mode so keys like Ctrl-C reach the guest,
/// and restores it on drop. A no-op when stdin is not a terminal.
struct RawTerminal(Option<Termios>);

impl RawTerminal {
    fn enter() -> Result<Self> {
        let stdin = io::stdin();
        if !stdin.is_terminal() {
            return Ok(Self(None));
        }
        Ok(Self(Some(make_raw(stdin.as_fd())?)))
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        if let Some(original) = &self.0 {
            let _ = tcsetattr(io::stdin().as_fd(), OptionalActions::Now, original);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_forwarding_at_detach_key() {
        let dir = std::env::temp_dir().join(format!("microvmctl-console-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pty");
        let sink = File::create(&path).unwrap();

        forward_input(&b"uname -a\n\x1dnot forwarded"[..], &sink).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"uname -a\n");

        std::fs::remove_dir_all(dir).unwrap();
    }
}
