use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct UnitState {
    pub id: String,
    pub active_state: String,
    pub sub_state: String,
    pub main_pid: Option<u32>,
    pub active_since: Option<u64>,
    pub memory_current: Option<u64>,
    pub cpu_usage_nsec: Option<u64>,
    pub restarts: u32,
    pub control_group: Option<String>,
}

impl UnitState {
    pub fn is_running(&self) -> bool {
        self.active_state == "active" && self.main_pid.is_some()
    }

    /// `running` when active, otherwise the high-level state (`inactive`, `failed`, ...).
    pub fn summary(&self) -> &str {
        if self.active_state == "active" {
            &self.sub_state
        } else {
            &self.active_state
        }
    }

    pub fn uptime(&self) -> Option<Duration> {
        if self.active_state != "active" {
            return None;
        }
        let since = UNIX_EPOCH + Duration::from_secs(self.active_since?);
        SystemTime::now().duration_since(since).ok()
    }
}

const PROPERTIES: &str = "Id,ActiveState,SubState,MainPID,ActiveEnterTimestamp,MemoryCurrent,CPUUsageNSec,NRestarts,ControlGroup";

pub fn show(units: &[String]) -> Result<Vec<UnitState>> {
    if units.is_empty() {
        return Ok(Vec::new());
    }
    let output = Command::new("systemctl")
        .args(["show", "--timestamp=unix", "--property", PROPERTIES])
        .args(units)
        .output()
        .context("running systemctl")?;
    if !output.status.success() {
        bail!(
            "systemctl show failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let by_id: HashMap<String, UnitState> = parse_show(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .map(|u| (u.id.clone(), u))
        .collect();
    Ok(units
        .iter()
        .map(|u| {
            by_id.get(u).cloned().unwrap_or_else(|| UnitState {
                id: u.clone(),
                ..Default::default()
            })
        })
        .collect())
}

fn parse_show(output: &str) -> Vec<UnitState> {
    output
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .map(|block| {
            let mut unit = UnitState::default();
            for (key, value) in block.lines().filter_map(|l| l.split_once('=')) {
                match key {
                    "Id" => unit.id = value.into(),
                    "ActiveState" => unit.active_state = value.into(),
                    "SubState" => unit.sub_state = value.into(),
                    "MainPID" => unit.main_pid = number(value).filter(|&p| p != 0),
                    "ActiveEnterTimestamp" => {
                        unit.active_since = value.strip_prefix('@').and_then(number)
                    }
                    "MemoryCurrent" => unit.memory_current = number(value),
                    "CPUUsageNSec" => unit.cpu_usage_nsec = number(value),
                    "NRestarts" => unit.restarts = number(value).unwrap_or(0),
                    "ControlGroup" => {
                        unit.control_group = Some(value.to_string()).filter(|v| !v.is_empty())
                    }
                    _ => {}
                }
            }
            unit
        })
        .collect()
}

/// systemd reports unset numeric properties as `[not set]` or `u64::MAX`.
fn number<T: std::str::FromStr + TryFrom<u64>>(value: &str) -> Option<T> {
    let n: u64 = value.parse().ok().filter(|&n| n != u64::MAX)?;
    T::try_from(n).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = "\
Id=microvm@osprey.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=@1793392791
MainPID=2662
NRestarts=0
ControlGroup=/system.slice/system-microvm.slice/microvm@osprey.service
MemoryCurrent=5179969536
CPUUsageNSec=3283015129000

Id=microvm-virtiofsd@osprey.service
ActiveState=inactive
SubState=dead
ActiveEnterTimestamp=
MainPID=0
NRestarts=0
ControlGroup=
MemoryCurrent=[not set]
CPUUsageNSec=18446744073709551615
";

    #[test]
    fn parses_running_and_dead_units() {
        let units = parse_show(OUTPUT);
        assert_eq!(units.len(), 2);

        let vm = &units[0];
        assert_eq!(vm.id, "microvm@osprey.service");
        assert_eq!(vm.summary(), "running");
        assert_eq!(vm.main_pid, Some(2662));
        assert_eq!(vm.active_since, Some(1793392791));
        assert_eq!(vm.memory_current, Some(5179969536));
        assert!(vm.is_running());

        let fs = &units[1];
        assert_eq!(fs.summary(), "inactive");
        assert_eq!(fs.main_pid, None);
        assert_eq!(fs.memory_current, None);
        assert_eq!(fs.cpu_usage_nsec, None);
        assert_eq!(fs.control_group, None);
        assert!(!fs.is_running());
    }
}
