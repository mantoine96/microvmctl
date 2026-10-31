pub mod api;
pub mod console;
pub mod control;
pub mod list;
pub mod logs;
pub mod ssh;
pub mod status;
pub mod top;

use crate::ch::{Client, Unavailable};
use crate::host::{self, RuntimeArgs};
use crate::systemd::{self, UnitState};
use crate::vm::Vm;
use anyhow::Result;

/// Everything about a VM that does not need the API, plus API access if we
/// have it.
pub struct Snapshot {
    pub vm: Vm,
    pub unit: UnitState,
    /// From the live process when running, else what `current` will boot with.
    pub args: Option<RuntimeArgs>,
    pub rss: Option<u64>,
    pub api: Result<Client, Unavailable>,
}

pub fn snapshot(vms: Vec<Vm>) -> Result<Vec<Snapshot>> {
    let units = systemd::show(&vms.iter().map(Vm::unit).collect::<Vec<_>>())?;
    Ok(vms
        .into_iter()
        .zip(units)
        .map(|(vm, unit)| {
            let pid = unit.main_pid.filter(|_| unit.is_running());
            Snapshot {
                args: match pid {
                    Some(pid) => host::runtime_args(pid),
                    None => vm.current().as_deref().and_then(host::configured_args),
                },
                rss: pid.and_then(host::rss),
                api: if pid.is_some() {
                    Client::probe(&vm)
                } else {
                    Err(Unavailable::NotRunning)
                },
                vm,
                unit,
            }
        })
        .collect())
}

impl Snapshot {
    /// The systemd state, refined with the hypervisor's own view when it
    /// differs (a paused VM is still `running` to systemd).
    pub fn state(&self, vm_state: Option<&str>) -> String {
        match vm_state {
            Some(s) if self.unit.is_running() && s != "Running" => s.to_lowercase(),
            _ => self.unit.summary().to_string(),
        }
    }
}
