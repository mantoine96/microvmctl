//! Point-in-time VM samples and the rates between two of them, shared by
//! `list`, `top` and the TUI.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::ch::{self, Counters, IoTotals, Unavailable, VmInfo};
use crate::cmd::snapshot;
use crate::{host, vm};

#[derive(Debug, Clone)]
pub struct Sample {
    pub at: Instant,
    pub name: String,
    pub state: String,
    pub pid: Option<u32>,
    pub uptime: Option<Duration>,
    pub vcpus: Option<u32>,
    pub memory: Option<u64>,
    pub rss: Option<u64>,
    pub cpu_time: Option<Duration>,
    pub cpu_usec: Option<u64>,
    pub io: Option<IoTotals>,
    pub restarts: u32,
    pub address: Option<String>,
    pub outdated: bool,
    /// The API exists but needs root, so `io` and paused state are unknown.
    pub api_denied: bool,
}

impl Sample {
    /// Conditions worth acting on, e.g. `outdated,restarted`.
    pub fn notes(&self) -> String {
        [
            self.outdated.then_some("outdated"),
            (self.restarts > 0).then_some("restarted"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(",")
    }
}

pub fn collect(state_dir: &Path) -> Result<Vec<Sample>> {
    let neighbors = host::neighbors();
    Ok(snapshot(vm::discover(state_dir)?)?
        .into_iter()
        .map(|s| {
            let api = s.api.as_ref().ok();
            let vm_state = api
                .and_then(|c| c.get::<VmInfo>("vm.info").ok())
                .map(|i| i.state);
            let args = s.args.as_ref();
            let cgroup = s
                .unit
                .control_group
                .as_deref()
                .filter(|_| s.unit.is_running());
            Sample {
                at: Instant::now(),
                state: s.state(vm_state.as_deref()),
                pid: s.unit.main_pid,
                uptime: s.unit.uptime(),
                vcpus: args.and_then(|a| a.vcpus),
                memory: args.and_then(|a| a.memory),
                rss: s.rss,
                cpu_time: s.unit.cpu_usage_nsec.map(Duration::from_nanos),
                cpu_usec: cgroup.and_then(host::cgroup_cpu_usec),
                io: api
                    .and_then(|c| c.get::<Counters>("vm.counters").ok())
                    .map(|c| ch::io_totals(&c)),
                restarts: s.unit.restarts,
                address: args
                    .into_iter()
                    .flat_map(|a| &a.nets)
                    .filter_map(|n| n.mac.as_deref())
                    .find_map(|mac| {
                        host::addresses_for(&neighbors, mac)
                            .first()
                            .map(|n| n.ip.clone())
                    }),
                outdated: s.vm.outdated(),
                api_denied: matches!(s.api, Err(Unavailable::PermissionDenied)),
                name: s.vm.name,
            }
        })
        .collect())
}

/// Per-second rates; `None` where either sample lacks the counter.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Rates {
    /// Percent of one host CPU, like `top`.
    pub cpu: Option<f64>,
    pub disk_read: Option<f64>,
    pub disk_write: Option<f64>,
    pub net_rx: Option<f64>,
    pub net_tx: Option<f64>,
}

pub fn rates(prev: Option<&Sample>, now: &Sample) -> Rates {
    let Some(prev) = prev else {
        return Rates::default();
    };
    let secs = now.at.duration_since(prev.at).as_secs_f64();
    if secs <= 0.0 {
        return Rates::default();
    }
    let per_sec = |a: u64, b: u64| b.saturating_sub(a) as f64 / secs;
    let io = |f: fn(&IoTotals) -> u64| match (prev.io, now.io) {
        (Some(a), Some(b)) => Some(per_sec(f(&a), f(&b))),
        _ => None,
    };
    Rates {
        cpu: match (prev.cpu_usec, now.cpu_usec) {
            (Some(a), Some(b)) => Some(per_sec(a, b) / 1e4),
            _ => None,
        },
        disk_read: io(|t| t.disk_read),
        disk_write: io(|t| t.disk_write),
        net_rx: io(|t| t.net_rx),
        net_tx: io(|t| t.net_tx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: Instant, cpu_usec: u64, disk_read: u64) -> Sample {
        Sample {
            at,
            name: "osprey".into(),
            state: "running".into(),
            pid: Some(1),
            uptime: None,
            vcpus: None,
            memory: None,
            rss: None,
            cpu_time: None,
            cpu_usec: Some(cpu_usec),
            io: Some(IoTotals {
                disk_read,
                ..Default::default()
            }),
            restarts: 0,
            address: None,
            outdated: false,
            api_denied: false,
        }
    }

    #[test]
    fn computes_rates_between_samples() {
        let t0 = Instant::now();
        let before = sample(t0, 1_000_000, 0);
        let after = sample(t0 + Duration::from_secs(2), 2_000_000, 4096);

        let r = rates(Some(&before), &after);
        assert_eq!(r.cpu, Some(50.0));
        assert_eq!(r.disk_read, Some(2048.0));
        assert_eq!(rates(None, &after), Rates::default());
    }
}
