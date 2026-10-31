use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::ch::{Counters, Unavailable, VmInfo};
use crate::fmt::{self, opt};
use crate::host::{self, Neighbor, RuntimeArgs};
use crate::systemd::{self, UnitState};
use crate::vm::Vm;

#[derive(Serialize)]
struct Status<'a> {
    name: &'a str,
    state: String,
    hypervisor: Option<String>,
    flake: Option<String>,
    booted: Option<String>,
    current: Option<String>,
    outdated: bool,
    unit: &'a UnitState,
    virtiofsd: Option<UnitState>,
    runtime_args: Option<&'a RuntimeArgs>,
    addresses: Vec<&'a Neighbor>,
    /// Raw `vm.info`, `null` when the API is unreachable (not running, or not root).
    vm_info: Option<Value>,
    vm_counters: Option<Counters>,
}

pub fn run(vm: &Vm, json: bool) -> Result<()> {
    print!("{}", report(vm, json)?);
    Ok(())
}

/// The `status` output, as text or JSON; the TUI shows the same text.
pub fn report(vm: &Vm, json: bool) -> Result<String> {
    let mut out = String::new();
    let snap = super::snapshot(vec![vm.clone()])?.remove(0);
    let generation = vm.booted().or_else(|| vm.current());
    let virtiofsd = match &generation {
        Some(g) if Vm::uses_virtiofs(g) => Some(systemd::show(&[vm.virtiofsd_unit()])?.remove(0)),
        _ => None,
    };
    let neighbors = host::neighbors();

    let (raw_info, counters) = match &snap.api {
        Ok(client) => (
            client.get::<Value>("vm.info").ok(),
            client.get::<Counters>("vm.counters").ok(),
        ),
        Err(_) => (None, None),
    };
    let info: Option<VmInfo> = raw_info
        .clone()
        .and_then(|v| serde_json::from_value(v).ok());

    let macs: Vec<&str> = snap
        .args
        .iter()
        .flat_map(|a| &a.nets)
        .filter_map(|n| n.mac.as_deref())
        .collect();
    let status = Status {
        name: &vm.name,
        state: snap.state(info.as_ref().map(|i| i.state.as_str())),
        hypervisor: vm.hypervisor(),
        flake: vm.flake(),
        booted: vm.booted().map(|p| p.display().to_string()),
        current: vm.current().map(|p| p.display().to_string()),
        outdated: vm.outdated(),
        unit: &snap.unit,
        virtiofsd,
        runtime_args: snap.args.as_ref(),
        addresses: macs
            .iter()
            .flat_map(|m| host::addresses_for(&neighbors, m))
            .collect(),
        vm_info: raw_info,
        vm_counters: counters.clone(),
    };

    if json {
        return Ok(serde_json::to_string_pretty(&status)? + "\n");
    }

    let unit = &snap.unit;
    let mut header = format!("{}  {}", vm.name, status.state);
    if let Some(up) = unit.uptime() {
        header += &format!("  up {}", fmt::duration(up));
    }
    if let Some(pid) = unit.main_pid {
        header += &format!("  (pid {pid}, {} restarts)", unit.restarts);
    }
    let _ = writeln!(out, "{header}");

    macro_rules! field {
        ($k:expr, $v:expr $(,)?) => {
            let _ = writeln!(out, "  {:<11} {}", $k, $v);
        };
    }
    field!(
        "hypervisor",
        status
            .hypervisor
            .clone()
            .unwrap_or_else(|| "unknown".into()),
    );
    // `booted` only exists while running; a stopped VM boots `current` next.
    if let Some(booted) = &generation {
        field!("generation", booted.display().to_string());
        if let Some(system) = Vm::system_of(booted) {
            field!("system", system.display().to_string());
        }
    }
    if status.outdated
        && let Some(current) = vm.current()
    {
        let target = Vm::system_of(&current).unwrap_or(current);
        field!(
            "pending",
            format!("{} (restart to apply)", target.display()),
        );
    }
    if let Some(flake) = &status.flake {
        field!("flake", flake.clone());
    }

    let args = snap.args.as_ref();
    let vcpus = info
        .as_ref()
        .map(|i| i.config.cpus.boot_vcpus)
        .or(args.and_then(|a| a.vcpus));
    let memory = info
        .as_ref()
        .map(|i| i.config.memory.size)
        .or(args.and_then(|a| a.memory));
    let mut resources = vec![
        format!("{} vCPU", opt(vcpus, |v| v.to_string())),
        format!("{} memory", opt(memory, fmt::bytes)),
    ];
    if let Some(m) = info.as_ref().map(|i| &i.config.memory) {
        let flags: Vec<&str> = [
            m.shared.then_some("shared"),
            m.mergeable.then_some("ksm"),
            m.hugepages.then_some("hugepages"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !flags.is_empty() {
            resources[1] += &format!(" ({})", flags.join(", "));
        }
    }
    if let Some(rss) = snap.rss {
        resources.push(format!("{} RSS", fmt::bytes(rss)));
    }
    if let Some(cgroup) = unit.memory_current {
        resources.push(format!("{} cgroup (incl. page cache)", fmt::bytes(cgroup)));
    }
    if let Some(cpu) = unit.cpu_usage_nsec {
        resources.push(format!("{} CPU", fmt::duration(Duration::from_nanos(cpu))));
    }
    field!("resources", resources.join(", "));
    if let Some(v) = &status.virtiofsd {
        field!("virtiofsd", v.summary().to_string());
    }
    let addresses: Vec<String> = status
        .addresses
        .iter()
        .map(|n| format!("{} ({})", n.ip, n.state.join(",").to_lowercase()))
        .collect();
    field!(
        "address",
        if addresses.is_empty() {
            "unknown (not in host neighbor table)".into()
        } else {
            addresses.join(", ")
        },
    );

    match (&info, &snap.api) {
        (Some(info), _) => print_devices(
            &mut out,
            vm,
            info,
            counters.as_ref().unwrap_or(&Counters::new()),
        ),
        (None, Err(Unavailable::PermissionDenied)) => {
            let _ = writeln!(
                out,
                "\n  (run as root for VM state, disks, network devices and I/O counters)"
            );
        }
        _ => {}
    }
    Ok(out)
}

fn print_devices(out: &mut String, vm: &Vm, info: &VmInfo, counters: &Counters) {
    let counter = |id: &Option<String>, key: &str| {
        id.as_ref()
            .and_then(|id| counters.get(id))
            .and_then(|c| c.get(key))
            .map(|&v| fmt::bytes(v))
            .unwrap_or_else(|| "-".into())
    };

    let disks: Vec<Vec<String>> = info
        .config
        .disks
        .iter()
        .flatten()
        .map(|d| {
            let meta = d
                .path
                .as_ref()
                .and_then(|p| fs::metadata(vm.dir.join(p)).ok());
            vec![
                d.id.clone().unwrap_or_default(),
                opt(d.path.as_ref(), |p| p.display().to_string()),
                if d.readonly { "ro" } else { "rw" }.into(),
                opt(meta.as_ref(), |m| fmt::bytes(m.len())),
                // Images are sparse; allocated blocks show real disk usage.
                opt(meta.as_ref(), |m| fmt::bytes(m.blocks() * 512)),
                counter(&d.id, "read_bytes"),
                counter(&d.id, "write_bytes"),
            ]
        })
        .collect();
    if !disks.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{}",
            indent(&fmt::table(
                &["DISK", "PATH", "MODE", "SIZE", "ALLOC", "READ", "WRITTEN"],
                &disks
            ))
        );
    }

    let nets: Vec<Vec<String>> = info
        .config
        .net
        .iter()
        .flatten()
        .map(|n| {
            vec![
                n.id.clone().unwrap_or_default(),
                n.tap.clone().unwrap_or_default(),
                n.mac.clone().unwrap_or_default(),
                counter(&n.id, "rx_bytes"),
                counter(&n.id, "tx_bytes"),
            ]
        })
        .collect();
    if !nets.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{}",
            indent(&fmt::table(&["NET", "TAP", "MAC", "RX", "TX"], &nets))
        );
    }

    let shares: Vec<Vec<String>> = info
        .config
        .fs
        .iter()
        .flatten()
        .map(|f| {
            vec![
                f.id.clone().unwrap_or_default(),
                f.tag.clone(),
                f.socket.display().to_string(),
            ]
        })
        .collect();
    if !shares.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{}",
            indent(&fmt::table(&["FS", "TAG", "SOCKET"], &shares))
        );
    }

    if let Some(b) = &info.config.balloon {
        let _ = writeln!(out, "\n  balloon     {}", fmt::bytes(b.size));
    }
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
