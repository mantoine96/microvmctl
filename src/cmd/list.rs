use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::ch::VmInfo;
use crate::fmt::{self, opt};
use crate::host;
use crate::vm;

#[derive(Serialize)]
struct Entry {
    name: String,
    state: String,
    pid: Option<u32>,
    uptime_secs: Option<u64>,
    vcpus: Option<u32>,
    memory: Option<u64>,
    rss: Option<u64>,
    cpu_time_secs: Option<u64>,
    restarts: u32,
    address: Option<String>,
    outdated: bool,
}

pub fn run(state_dir: &Path, json: bool) -> Result<()> {
    let neighbors = host::neighbors();
    let entries: Vec<Entry> = super::snapshot(vm::discover(state_dir)?)?
        .into_iter()
        .map(|s| {
            let vm_state = s
                .api
                .as_ref()
                .ok()
                .and_then(|c| c.get::<VmInfo>("vm.info").ok())
                .map(|i| i.state);
            let args = s.args.as_ref();
            Entry {
                name: s.vm.name.clone(),
                state: s.state(vm_state.as_deref()),
                pid: s.unit.main_pid,
                uptime_secs: s.unit.uptime().map(|d| d.as_secs()),
                vcpus: args.and_then(|a| a.vcpus),
                memory: args.and_then(|a| a.memory),
                rss: s.rss,
                cpu_time_secs: s.unit.cpu_usage_nsec.map(|n| n / 1_000_000_000),
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
            }
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            vec![
                e.name.clone(),
                e.state.clone(),
                opt(e.pid, |p| p.to_string()),
                opt(e.uptime_secs, |s| fmt::duration(Duration::from_secs(s))),
                opt(e.vcpus, |v| v.to_string()),
                opt(e.memory, fmt::bytes),
                opt(e.rss, fmt::bytes),
                opt(e.cpu_time_secs, |s| fmt::duration(Duration::from_secs(s))),
                opt(e.address.clone(), |a| a),
                [
                    e.outdated.then_some("outdated"),
                    (e.restarts > 0).then_some("restarted"),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(","),
            ]
        })
        .collect();
    println!(
        "{}",
        fmt::table(
            &[
                "NAME", "STATE", "PID", "UPTIME", "VCPU", "MEM", "RSS", "CPU", "ADDRESS", "NOTES"
            ],
            &rows
        )
    );
    Ok(())
}
