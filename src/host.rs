//! Unprivileged host-side views of a running VM: its hypervisor command
//! line, cgroup accounting, and the host neighbor table.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// What we can learn from the cloud-hypervisor command line in `/proc`,
/// which (unlike the API socket) is readable without root.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct RuntimeArgs {
    pub vcpus: Option<u32>,
    pub memory: Option<u64>,
    pub nets: Vec<NetArg>,
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct NetArg {
    pub tap: Option<String>,
    pub mac: Option<String>,
}

pub fn runtime_args(pid: u32) -> Option<RuntimeArgs> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = raw
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    Some(parse_args(&args))
}

/// The arguments a generation will boot with, from the `exec` line of its
/// `bin/microvm-run`. Used for stopped VMs, which have no process to inspect.
pub fn configured_args(generation: &Path) -> Option<RuntimeArgs> {
    let script = fs::read_to_string(generation.join("bin/microvm-run")).ok()?;
    let line = script
        .lines()
        .find(|l| l.trim_start().starts_with("exec "))?;
    Some(parse_args(&shell_words(line)))
}

/// Splits a line the way the shell would for the quoting microvm.nix
/// emits (single and double quotes, backslash escapes). Expansions like
/// `${runtime_args:-}` are kept literally, which is harmless here.
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                word.extend(chars.by_ref().take_while(|&c| c != '\''));
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => word.extend(chars.next()),
                        _ => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                word.extend(chars.next());
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// cloud-hypervisor flags take one or more values (`--disk a b`), so a
/// flag's values run until the next `--flag`.
fn parse_args(args: &[String]) -> RuntimeArgs {
    let mut out = RuntimeArgs::default();
    let mut flag = "";
    for arg in args {
        if let Some(f) = arg.strip_prefix("--") {
            flag = f;
            continue;
        }
        match flag {
            "cpus" => out.vcpus = option(arg, "boot").and_then(|v| v.parse().ok()),
            "memory" => out.memory = option(arg, "size").and_then(parse_size),
            "net" => {
                let net = NetArg {
                    tap: option(arg, "tap").map(Into::into),
                    mac: option(arg, "mac").map(Into::into),
                };
                if net.tap.is_some() || net.mac.is_some() {
                    out.nets.push(net);
                }
            }
            _ => {}
        }
    }
    out
}

/// Extracts `key` from a `k1=v1,k2=v2` option string.
fn option<'a>(value: &'a str, key: &str) -> Option<&'a str> {
    value
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .find_map(|(k, v)| (k == key).then_some(v))
}

fn parse_size(s: &str) -> Option<u64> {
    let (digits, mult) = match s.chars().last()? {
        'K' | 'k' => (&s[..s.len() - 1], 1 << 10),
        'M' | 'm' => (&s[..s.len() - 1], 1 << 20),
        'G' | 'g' => (&s[..s.len() - 1], 1 << 30),
        _ => (s, 1),
    };
    digits.parse::<u64>().ok().map(|n| n * mult)
}

/// Cumulative CPU time of a unit's cgroup, in microseconds.
pub fn cgroup_cpu_usec(control_group: &str) -> Option<u64> {
    let stat = fs::read_to_string(
        Path::new("/sys/fs/cgroup")
            .join(control_group.trim_start_matches('/'))
            .join("cpu.stat"),
    )
    .ok()?;
    stat.lines()
        .find_map(|l| l.strip_prefix("usage_usec "))
        .and_then(|v| v.trim().parse().ok())
}

/// Resident memory of the hypervisor process. Unlike the unit's cgroup
/// memory, this excludes host page cache of the disk images, which can
/// exceed the guest's own size.
pub fn rss(pid: u32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    parse_rss(&status)
}

fn parse_rss(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().strip_suffix("kB"))
        .and_then(|kb| kb.trim().parse::<u64>().ok())
        .map(|kb| kb * 1024)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Neighbor {
    #[serde(rename = "dst")]
    pub ip: String,
    #[serde(rename = "lladdr", default)]
    pub mac: String,
    #[serde(default)]
    pub state: Vec<String>,
}

impl Neighbor {
    /// Lower is fresher. `None` for entries that do not prove the address.
    fn rank(&self) -> Option<u8> {
        ["PERMANENT", "REACHABLE", "DELAY", "PROBE", "STALE"]
            .iter()
            .position(|s| self.state.iter().any(|st| st == s))
            .map(|p| p as u8)
    }
}

/// Best-effort guest addresses: whatever the host has recently seen on the
/// wire for this MAC. Empty if the guest has not talked to the host.
pub fn neighbors() -> Vec<Neighbor> {
    Command::new("ip")
        .args(["-json", "neigh", "show"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice(&o.stdout).ok())
        .unwrap_or_default()
}

pub fn addresses_for<'a>(neighbors: &'a [Neighbor], mac: &str) -> Vec<&'a Neighbor> {
    let mut found: Vec<&Neighbor> = neighbors
        .iter()
        .filter(|n| n.mac.eq_ignore_ascii_case(mac) && n.rank().is_some())
        .collect();
    found.sort_by_key(|n| n.rank());
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn parses_cloud_hypervisor_cmdline() {
        let parsed = parse_args(&args(&[
            "microvm@falcon",
            "--cpus",
            "boot=4",
            "--watchdog",
            "--cmdline",
            "console=ttyS0 init=/nix/store/x/init",
            "--memory",
            "shared=on,size=8192M",
            "--disk",
            "path=var.img,readonly=off",
            "path=home.img",
            "--net",
            "mac=02:00:00:00:00:01,num_queues=8,tap=vm-falcon",
        ]));
        assert_eq!(parsed.vcpus, Some(4));
        assert_eq!(parsed.memory, Some(8 << 30));
        assert_eq!(
            parsed.nets,
            [NetArg {
                tap: Some("vm-falcon".into()),
                mac: Some("02:00:00:00:00:01".into())
            }]
        );
    }

    #[test]
    fn splits_microvm_run_exec_line() {
        let line = r#"exec -a "microvm@osprey" /nix/store/x/bin/cloud-hypervisor --cpus 'boot=2' --memory 'mergeable=on,shared=off,size=2048M' --cmdline 'console=ttyS0 init=/nix/store/y/init' --net 'mac=02:00:00:00:00:02,num_queues=4,tap=vm-osprey'   ${runtime_args:-}"#;
        let words = shell_words(line);
        assert_eq!(words[..3], ["exec", "-a", "microvm@osprey"]);
        assert!(words.contains(&"console=ttyS0 init=/nix/store/y/init".to_string()));

        let parsed = parse_args(&words);
        assert_eq!(parsed.vcpus, Some(2));
        assert_eq!(parsed.memory, Some(2 << 30));
        assert_eq!(parsed.nets[0].tap.as_deref(), Some("vm-osprey"));
    }

    #[test]
    fn parses_vm_rss() {
        let status = "Name:\tcloud-hyperviso\nVmRSS:\t 2098028 kB\nRssAnon:\t 2094316 kB\n";
        assert_eq!(parse_rss(status), Some(2098028 * 1024));
        assert_eq!(parse_rss("Name:\tkthreadd\n"), None);
    }

    #[test]
    fn prefers_fresh_neighbor_entries() {
        let table: Vec<Neighbor> = serde_json::from_str(
            r#"[{"dst":"10.100.10.19","dev":"br0","lladdr":"02:00:00:00:00:01","state":["STALE"]},
                {"dst":"10.100.10.16","dev":"br0","lladdr":"02:00:00:00:00:01","state":["REACHABLE"]},
                {"dst":"10.100.10.40","dev":"br0","lladdr":"02:00:00:00:00:01","state":["FAILED"]},
                {"dst":"10.100.10.1","dev":"br0","lladdr":"02:00:00:00:00:fe","state":["REACHABLE"]},
                {"dst":"10.100.10.99","dev":"br0","state":["INCOMPLETE"]}]"#,
        )
        .unwrap();
        let ips: Vec<&str> = addresses_for(&table, "02:00:00:00:00:01")
            .iter()
            .map(|n| n.ip.as_str())
            .collect();
        assert_eq!(ips, ["10.100.10.16", "10.100.10.19"]);
    }
}
