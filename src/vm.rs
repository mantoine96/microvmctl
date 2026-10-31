use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub const DEFAULT_STATE_DIR: &str = "/var/lib/microvms";

/// A microvm.nix VM, identified by its state directory `<state_dir>/<name>`.
#[derive(Debug, Clone)]
pub struct Vm {
    pub name: String,
    pub dir: PathBuf,
}

impl Vm {
    pub fn unit(&self) -> String {
        format!("microvm@{}.service", self.name)
    }

    pub fn virtiofsd_unit(&self) -> String {
        format!("microvm-virtiofsd@{}.service", self.name)
    }

    pub fn api_socket(&self) -> PathBuf {
        self.dir.join(format!("{}.sock", self.name))
    }

    /// The generation the host config wants to run.
    pub fn current(&self) -> Option<PathBuf> {
        fs::read_link(self.dir.join("current")).ok()
    }

    /// The generation the VM was last started with.
    pub fn booted(&self) -> Option<PathBuf> {
        fs::read_link(self.dir.join("booted")).ok()
    }

    /// True when the host was rebuilt with a new VM generation but the VM
    /// still runs the old one (microvm.nix units have `X-RestartIfChanged=false`).
    pub fn outdated(&self) -> bool {
        matches!((self.current(), self.booted()), (Some(c), Some(b)) if c != b)
    }

    pub fn hypervisor(&self) -> Option<String> {
        self.share_file("hypervisor")
    }

    /// The NixOS system toplevel of the given generation.
    ///
    /// microvm.nix only links `share/microvm/system` when the store is
    /// shared over virtiofs, so fall back to the kernel's `init=` argument
    /// baked into the run script (e.g. VMs booting from an erofs store disk).
    pub fn system_of(generation: &Path) -> Option<PathBuf> {
        fs::read_link(generation.join("share/microvm/system"))
            .ok()
            .or_else(|| {
                let script = fs::read_to_string(generation.join("bin/microvm-run")).ok()?;
                system_from_run_script(&script)
            })
    }

    /// Whether the generation runs virtiofsd, i.e. `microvm-virtiofsd@` matters.
    pub fn uses_virtiofs(generation: &Path) -> bool {
        generation.join("bin/virtiofsd-run").exists()
    }

    pub fn flake(&self) -> Option<String> {
        read_trimmed(&self.dir.join("flake"))
    }

    /// Read from the running generation if any: a host rebuild may have
    /// switched `current` to a different hypervisor than the one running.
    fn share_file(&self, name: &str) -> Option<String> {
        let generation = self.booted().or_else(|| self.current())?;
        read_trimmed(&generation.join("share/microvm").join(name))
    }
}

fn system_from_run_script(script: &str) -> Option<PathBuf> {
    let start = script.find(" init=/nix/store/")? + " init=".len();
    let init: String = script[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '\'' && *c != '"')
        .collect();
    Some(PathBuf::from(init.strip_suffix("/init")?))
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn discover(state_dir: &Path) -> Result<Vec<Vm>> {
    let entries =
        fs::read_dir(state_dir).with_context(|| format!("reading {}", state_dir.display()))?;
    let mut vms: Vec<Vm> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("current/bin/microvm-run").exists())
        .filter_map(|e| {
            Some(Vm {
                name: e.file_name().into_string().ok()?,
                dir: e.path(),
            })
        })
        .collect();
    vms.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(vms)
}

pub fn find(state_dir: &Path, name: &str) -> Result<Vm> {
    let vms = discover(state_dir)?;
    if let Some(vm) = vms.iter().find(|vm| vm.name == name) {
        return Ok(vm.clone());
    }
    let known: Vec<&str> = vms.iter().map(|vm| vm.name.as_str()).collect();
    bail!(
        "no VM named {name:?} in {} (known: {})",
        state_dir.display(),
        if known.is_empty() {
            "none".to_string()
        } else {
            known.join(", ")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("microvmctl-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_generation(root: &Path, name: &str) -> PathBuf {
        let generation = root.join("store").join(name);
        fs::create_dir_all(generation.join("bin")).unwrap();
        fs::create_dir_all(generation.join("share/microvm")).unwrap();
        fs::write(generation.join("bin/microvm-run"), "").unwrap();
        fs::write(
            generation.join("share/microvm/hypervisor"),
            "cloud-hypervisor\n",
        )
        .unwrap();
        generation
    }

    #[test]
    fn extracts_system_from_run_script() {
        let script = "exec -a \"microvm@osprey\" /nix/store/x-cloud-hypervisor-52.0/bin/cloud-hypervisor \
            --cmdline 'console=ttyS0 root=fstab init=/nix/store/ay32-nixos-system-osprey-26.05/init regInfo=/nix/store/sz-closure-info/registration' \
            --seccomp true";
        assert_eq!(
            system_from_run_script(script),
            Some(PathBuf::from("/nix/store/ay32-nixos-system-osprey-26.05"))
        );
        assert_eq!(system_from_run_script("exec qemu --nographic"), None);
    }

    #[test]
    fn discovers_vms_and_detects_outdated_generation() {
        let root = scratch("discover");
        let state = root.join("microvms");
        let gen_a = make_generation(&root, "gen-a");
        let gen_b = make_generation(&root, "gen-b");

        for (vm, current, booted) in [("osprey", &gen_a, &gen_a), ("falcon", &gen_b, &gen_a)] {
            let dir = state.join(vm);
            fs::create_dir_all(&dir).unwrap();
            symlink(current, dir.join("current")).unwrap();
            symlink(booted, dir.join("booted")).unwrap();
        }
        fs::create_dir_all(state.join("not-a-vm")).unwrap();

        let vms = discover(&state).unwrap();
        let names: Vec<&str> = vms.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["falcon", "osprey"]);
        assert!(vms[0].outdated());
        assert!(!vms[1].outdated());
        assert_eq!(vms[1].hypervisor().as_deref(), Some("cloud-hypervisor"));
        assert_eq!(vms[1].api_socket(), state.join("osprey/osprey.sock"));

        let err = find(&state, "nope").unwrap_err().to_string();
        assert!(err.contains("known: falcon, osprey"), "{err}");

        fs::remove_dir_all(root).unwrap();
    }
}
