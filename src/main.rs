mod ch;
mod cmd;
mod fmt;
mod host;
mod metrics;
mod systemd;
mod vm;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use cmd::control::Transition;

/// Manage and observe microvm.nix cloud-hypervisor VMs.
///
/// Read-only commands work unprivileged. Commands that talk to the
/// cloud-hypervisor API socket (pause, resume, console, ssh, api, and the device details of
/// status/top) need root, because microvm.nix creates the socket owner-only.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Directory holding one subdirectory per VM.
    #[arg(long, global = true, env = "MICROVMS_STATE_DIR", default_value = vm::DEFAULT_STATE_DIR)]
    state_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List VMs with their state and resource usage.
    #[command(visible_alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show details of one VM: generation, resources, disks, network.
    Status {
        vm: String,
        #[arg(long)]
        json: bool,
    },
    /// Show the VM's journal (guest serial console and hypervisor output).
    Logs(cmd::logs::Args),
    /// Start VMs through systemd.
    Start {
        #[arg(required = true)]
        vms: Vec<String>,
    },
    /// Stop VMs through systemd (sends an ACPI power-button press).
    Stop {
        #[arg(required = true)]
        vms: Vec<String>,
    },
    /// Restart VMs through systemd, e.g. to apply a new generation.
    Restart {
        #[arg(required = true)]
        vms: Vec<String>,
    },
    /// Freeze the guest's vCPUs.
    Pause { vm: String },
    /// Resume a paused guest.
    Resume { vm: String },
    /// Live per-VM CPU, memory, disk and network rates.
    Top(cmd::top::Args),
    /// Attach to the guest console (needs `--console pty`), Ctrl-] to detach.
    Console { vm: String },
    /// SSH into the guest over vsock, without needing guest networking.
    Ssh(cmd::ssh::Args),
    /// ssh ProxyCommand helper used by `microvmctl ssh`.
    #[command(hide = true)]
    VsockProxy(cmd::ssh::ProxyArgs),
    /// Call a raw cloud-hypervisor API endpoint, e.g. `vm.info` or `vm.counters`.
    Api(cmd::api::Args),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = &cli.state_dir;
    match cli.command {
        Command::List { json } => cmd::list::run(dir, json),
        Command::Status { vm, json } => cmd::status::run(&vm::find(dir, &vm)?, json),
        Command::Logs(args) => cmd::logs::run(dir, args),
        Command::Start { vms } => cmd::control::systemctl(dir, "start", &vms),
        Command::Stop { vms } => cmd::control::systemctl(dir, "stop", &vms),
        Command::Restart { vms } => cmd::control::systemctl(dir, "restart", &vms),
        Command::Pause { vm } => cmd::control::transition(&vm::find(dir, &vm)?, Transition::Pause),
        Command::Resume { vm } => {
            cmd::control::transition(&vm::find(dir, &vm)?, Transition::Resume)
        }
        Command::Top(args) => cmd::top::run(dir, args),
        Command::Api(args) => cmd::api::run(dir, args),
        Command::Console { vm } => cmd::console::run(&vm::find(dir, &vm)?),
        Command::Ssh(args) => cmd::ssh::run(dir, args),
        Command::VsockProxy(args) => cmd::ssh::proxy(args),
    }
}
