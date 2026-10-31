use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::fmt::{self, opt};
use crate::metrics;

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
    let samples = metrics::collect(state_dir)?;
    let notes: Vec<String> = samples.iter().map(metrics::Sample::notes).collect();
    let entries: Vec<Entry> = samples
        .into_iter()
        .map(|s| Entry {
            name: s.name,
            state: s.state,
            pid: s.pid,
            uptime_secs: s.uptime.map(|d| d.as_secs()),
            vcpus: s.vcpus,
            memory: s.memory,
            rss: s.rss,
            cpu_time_secs: s.cpu_time.map(|d| d.as_secs()),
            restarts: s.restarts,
            address: s.address,
            outdated: s.outdated,
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    let rows: Vec<Vec<String>> = entries
        .iter()
        .zip(notes)
        .map(|(e, notes)| {
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
                notes,
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
