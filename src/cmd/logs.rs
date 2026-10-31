use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::{systemd, vm};

#[derive(clap::Args)]
pub struct Args {
    vm: String,
    /// Follow new entries.
    #[arg(short, long)]
    follow: bool,
    /// Number of most recent entries to show.
    #[arg(short = 'n', long)]
    lines: Option<u32>,
    /// Show entries since this time (any `journalctl --since` format).
    #[arg(long, conflicts_with = "current")]
    since: Option<String>,
    /// Only the current run of the VM, since its unit last started.
    #[arg(short, long)]
    current: bool,
    /// Include the helper units (virtiofsd, tap setup, ...).
    #[arg(long)]
    helpers: bool,
    /// Extra arguments passed to journalctl, after `--`.
    #[arg(last = true)]
    extra: Vec<String>,
}

pub fn run(state_dir: &Path, args: Args) -> Result<()> {
    let vm = vm::find(state_dir, &args.vm)?;
    let mut cmd = Command::new("journalctl");
    // The guest serial console emits ANSI colors, which journalctl would
    // otherwise collapse to `[92B blob data]`.
    cmd.arg("--all");
    if args.helpers {
        // Matches microvm@ and every microvm-<helper>@ unit of this VM.
        cmd.arg(format!("--unit=microvm*@{}.service", vm.name));
    } else {
        cmd.arg(format!("--unit={}", vm.unit()));
    }
    if args.follow {
        cmd.arg("--follow");
    }
    if let Some(n) = args.lines {
        cmd.arg(format!("--lines={n}"));
    }
    if let Some(since) = args.since {
        cmd.arg(format!("--since={since}"));
    }
    if args.current {
        let unit = systemd::show(&[vm.unit()])?.remove(0);
        let since = unit
            .active_since
            .with_context(|| format!("{} has not started yet", vm.unit()))?;
        cmd.arg(format!("--since=@{since}"));
    }
    cmd.args(&args.extra);
    Err(cmd.exec()).context("executing journalctl")
}
