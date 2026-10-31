use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::ch::{Client, VmInfo};
use crate::vm::{self, Vm};

/// Hands over to systemctl so its own polkit prompts and job output apply.
pub fn systemctl(state_dir: &Path, verb: &str, names: &[String]) -> Result<()> {
    let units = names
        .iter()
        .map(|n| vm::find(state_dir, n).map(|vm| vm.unit()))
        .collect::<Result<Vec<_>>>()?;
    Err(Command::new("systemctl").arg(verb).args(units).exec()).context("executing systemctl")
}

pub enum Transition {
    Pause,
    Resume,
}

/// Idempotent like `systemctl start`: cloud-hypervisor rejects pausing a
/// paused VM with an opaque state-machine error, so check first.
pub fn transition(vm: &Vm, transition: Transition) -> Result<()> {
    let (endpoint, target) = match transition {
        Transition::Pause => ("vm.pause", "Paused"),
        Transition::Resume => ("vm.resume", "Running"),
    };
    let client = Client::for_vm(vm, endpoint.trim_start_matches("vm."))?;
    if client.get::<VmInfo>("vm.info")?.state == target {
        eprintln!("{} is already {}", vm.name, target.to_lowercase());
        return Ok(());
    }
    client.put(endpoint, None)
}
