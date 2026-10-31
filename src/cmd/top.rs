use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::Result;

use crate::fmt::{self, opt};
use crate::metrics::{self, Sample};

#[derive(clap::Args)]
pub struct Args {
    /// Seconds between refreshes.
    #[arg(short = 'd', long, default_value_t = 2.0)]
    delay: f64,
    /// Stop after this many refreshes (0 means run until interrupted).
    #[arg(short = 'n', long, default_value_t = 0)]
    iterations: u64,
}

fn by_name(samples: Vec<Sample>) -> HashMap<String, Sample> {
    samples.into_iter().map(|s| (s.name.clone(), s)).collect()
}

pub fn run(state_dir: &Path, args: Args) -> Result<()> {
    let delay = Duration::from_secs_f64(args.delay.max(0.1));
    let interactive = std::io::stdout().is_terminal();
    let mut previous = by_name(metrics::collect(state_dir)?);

    for iteration in 1.. {
        thread::sleep(delay);
        let current = metrics::collect(state_dir)?;
        let denied = current.iter().any(|s| s.api_denied);

        let rows: Vec<Vec<String>> = current
            .iter()
            .map(|now| {
                let r = metrics::rates(previous.get(&now.name), now);
                vec![
                    now.name.clone(),
                    now.state.clone(),
                    opt(r.cpu, |c| format!("{c:.1}")),
                    opt(now.rss, fmt::bytes),
                    opt(r.disk_read, fmt::rate),
                    opt(r.disk_write, fmt::rate),
                    opt(r.net_rx, fmt::rate),
                    opt(r.net_tx, fmt::rate),
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

        previous = by_name(current);
        if args.iterations != 0 && iteration >= args.iterations {
            break;
        }
    }
    Ok(())
}
