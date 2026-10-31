use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::ch::{self, Counters, Unavailable};
use crate::fmt::{self, opt};
use crate::{host, vm};

#[derive(clap::Args)]
pub struct Args {
    /// Seconds between refreshes.
    #[arg(short = 'd', long, default_value_t = 2.0)]
    delay: f64,
    /// Stop after this many refreshes (0 means run until interrupted).
    #[arg(short = 'n', long, default_value_t = 0)]
    iterations: u64,
}

struct Sample {
    at: Instant,
    state: String,
    cpu_usec: Option<u64>,
    rss: Option<u64>,
    io: Option<ch::IoTotals>,
    denied: bool,
}

fn sample_all(state_dir: &Path) -> Result<Vec<(String, Sample)>> {
    Ok(super::snapshot(vm::discover(state_dir)?)?
        .into_iter()
        .map(|s| {
            let cgroup = s
                .unit
                .control_group
                .as_deref()
                .filter(|_| s.unit.is_running());
            let sample = Sample {
                at: Instant::now(),
                state: s.unit.summary().to_string(),
                cpu_usec: cgroup.and_then(host::cgroup_cpu_usec),
                rss: s.rss,
                io: s
                    .api
                    .as_ref()
                    .ok()
                    .and_then(|c| c.get::<Counters>("vm.counters").ok())
                    .map(|c| ch::io_totals(&c)),
                denied: matches!(s.api, Err(Unavailable::PermissionDenied)),
            };
            (s.vm.name, sample)
        })
        .collect())
}

pub fn run(state_dir: &Path, args: Args) -> Result<()> {
    let delay = Duration::from_secs_f64(args.delay.max(0.1));
    let interactive = std::io::stdout().is_terminal();
    let mut previous: HashMap<String, Sample> = sample_all(state_dir)?.into_iter().collect();

    for iteration in 1.. {
        thread::sleep(delay);
        let current = sample_all(state_dir)?;
        let denied = current.iter().any(|(_, s)| s.denied);

        let rows: Vec<Vec<String>> = current
            .iter()
            .map(|(name, now)| {
                let prev = previous.get(name);
                let secs = prev.map(|p| now.at.duration_since(p.at).as_secs_f64());
                let per_sec = |f: fn(&ch::IoTotals) -> u64| -> String {
                    match (prev.and_then(|p| p.io), now.io, secs) {
                        (Some(a), Some(b), Some(s)) => {
                            fmt::rate(f(&b).saturating_sub(f(&a)) as f64 / s)
                        }
                        _ => "-".into(),
                    }
                };
                let cpu = match (prev.and_then(|p| p.cpu_usec), now.cpu_usec, secs) {
                    (Some(a), Some(b), Some(s)) => Some(b.saturating_sub(a) as f64 / (s * 1e4)),
                    _ => None,
                };
                vec![
                    name.clone(),
                    now.state.clone(),
                    opt(cpu, |c| format!("{c:.1}")),
                    opt(now.rss, fmt::bytes),
                    per_sec(|t| t.disk_read),
                    per_sec(|t| t.disk_write),
                    per_sec(|t| t.net_rx),
                    per_sec(|t| t.net_tx),
                ]
            })
            .collect();

        let mut out = std::io::stdout().lock();
        if interactive {
            write!(out, "\x1b[H\x1b[2J")?;
        }
        writeln!(
            out,
            "{}",
            fmt::table(
                &[
                    "NAME", "STATE", "CPU%", "RSS", "DISK R/s", "DISK W/s", "NET RX/s", "NET TX/s"
                ],
                &rows
            )
        )?;
        if denied {
            writeln!(out, "\n(run as root for disk and network rates)")?;
        }
        if !interactive {
            writeln!(out)?;
        }
        out.flush()?;

        previous = current.into_iter().collect();
        if args.iterations != 0 && iteration >= args.iterations {
            break;
        }
    }
    Ok(())
}
