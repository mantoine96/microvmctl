# microvmctl

[![CI](https://github.com/mantoine96/microvmctl/actions/workflows/ci.yml/badge.svg)](https://github.com/mantoine96/microvmctl/actions/workflows/ci.yml)

Manage and observe [microvm.nix](https://github.com/microvm-nix/microvm.nix) VMs
running on [cloud-hypervisor](https://www.cloudhypervisor.org/),
without juggling `systemctl`, `journalctl`, `curl --unix-socket` and `readlink`.

```console
$ microvmctl ls
NAME    STATE    PID    UPTIME  VCPU  MEM   RSS   CPU     ADDRESS       NOTES
falcon  running  12430  4h28m   4     8.0G  8.0G  2h47m   10.100.10.16
osprey  running  2662   11h4m   2     2.0G  2.0G  55m16s  -             outdated
```

Run it without arguments and you get a live, k9s-style view of every VM on the host:

```text
 microvmctl   host: hypervisor-host  vms: 2  root
 <enter/d> describe <l> logs <a> attach console <s> ssh <u> start <D> stop <R> restart <p> pause/resume </> filter <?> help <q> quit
╭────────────────────────────────────────── VMs(all)[2] ───────────────────────────────────────────╮
│NAME     STATE     PID     UPTIME   VCPU   MEM    RSS    CPU%   DISK R/s   DISK W/s   NET RX/s ...│
│falcon   running   12430   5h1m     4      8.0G   8.0G   47.5   0B/s       362K/s     7.0K/s      │
│osprey   paused    2662    11h37m   2      2.0G   2.0G   0.0    0B/s       0B/s       0B/s        │
╰──────────────────────────────────────────────────────────────────────────────────────────────────╯
```

It runs on the VM host and works with the VMs microvm.nix already declares:
it reads `/var/lib/microvms`, the `microvm@<name>` systemd units,
and each VM's cloud-hypervisor API socket.
Nothing needs to change in your VM configuration for the basics.

## Install it on the host through the flake

Add the flake as an input of the host's NixOS configuration:

```nix
{
  inputs.microvmctl = {
    url = "github:mantoine96/microvmctl";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, microvmctl, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      modules = [
        ({ pkgs, ... }: {
          environment.systemPackages = [
            microvmctl.packages.${pkgs.stdenv.hostPlatform.system}.default
          ];
        })
      ];
    };
  };
}
```

An overlay is also available as `microvmctl.overlays.default`, providing `pkgs.microvmctl`.

To try it without installing anything: `nix run github:mantoine96/microvmctl -- ls`.

## Most commands work without root, actions need it

microvm.nix creates each VM's API socket as owner-only (`microvm:kvm`, mode `0700`).
Rather than asking you to loosen that, microvmctl splits its work:

- **Unprivileged:** `list`, `logs`, `top`, the TUI, and most of `status`.
  These read systemd, cgroups and `/proc`, and show `-` with a hint where root would add detail.
- **Root:** anything that talks to cloud-hypervisor itself.
  That covers `pause`, `resume`, `console`, `ssh` and `api`,
  plus the paused state, disk/network devices and I/O rates in `status`, `top` and the TUI.

`start`, `stop` and `restart` go through `systemctl`, so its usual polkit rules apply.
In practice, `sudo microvmctl` is the comfortable way to run the TUI.

## cloud-hypervisor gets full support, other backends the basics

Anything built on systemd and the state directory works for every microvm.nix backend.
Anything that talks to the hypervisor uses cloud-hypervisor's REST API,
so it refuses VMs on other backends with a clear message.

| Works for any backend | cloud-hypervisor only |
|---|---|
| `list`: state, PID, uptime, RSS, CPU time, `outdated`, restarts | vCPU and memory columns |
| `logs`, `start`, `stop`, `restart` | `pause`, `resume`, `console`, `ssh`, `api` |
| `status`: generation, NixOS system, flake, virtiofsd | `status`: disks, network, shares, paused state |
| `top` and the TUI: CPU% and RSS | Disk and network rates |

This has been tested with cloud-hypervisor and qemu.
Firecracker, crosvm, kvmtool, stratovirt, alioth and vfkit should behave like qemu here,
but have not been tried.

## Commands

| Command | What it does |
|---|---|
| `microvmctl` / `microvmctl tui` | Interactive view, see [below](#the-tui-follows-k9s-conventions) |
| `list` (`ls`) `[--json]` | State, uptime, vCPUs, memory, RSS, CPU time, guest address, notes |
| `status <vm> [--json]` | Generation, NixOS system, pending update, disks, network, virtiofs shares |
| `logs <vm> [-f] [-n N] [-c] [--since T] [--helpers] [-- journalctl args]` | Guest serial console and hypervisor output from the journal |
| `top [-d secs] [-n iterations]` | Live CPU%, RSS, disk and network rates |
| `start` / `stop` / `restart <vm>...` | Through systemd; `stop` presses the ACPI power button |
| `pause` / `resume <vm>` | Freeze or thaw the guest's vCPUs (idempotent) |
| `console <vm>` | Interactive guest console, Ctrl-] to detach |
| `ssh [user@]<vm> [-- cmd]` | SSH into the guest over vsock, no guest network needed |
| `api <vm> <endpoint> [-X METHOD] [-d JSON]` | Raw cloud-hypervisor API call, e.g. `vm.info` |

A few behaviours are worth knowing:

- **`outdated` means a host rebuild has not reached the VM yet.**
  microvm.nix marks its units `X-RestartIfChanged=false`,
  so after `nixos-rebuild` the VM keeps running the old generation.
  `status` shows the pending system, and `microvmctl restart <vm>` applies it.
- **RSS is the hypervisor process, not the unit's cgroup.**
  systemd's memory figure includes host page cache from the disk images
  and can exceed the guest's own size; `status` shows both, labelled.
- **Guest addresses are best effort.**
  They come from the host's neighbor table, matched on the VM's MAC address,
  so a guest that has not recently talked to the host shows `-`.
- **`logs --current` limits output to the VM's current run**,
  and `--helpers` adds the virtiofsd and network setup units.

## Console and SSH need one setting per VM

Both are off in microvm.nix by default.
If you try either on a VM that lacks the setting,
microvmctl prints the exact snippet to add.

### Console: a login prompt that survives broken networking

```nix
microvm.cloud-hypervisor.extraArgs = [ "--console" "pty" ];
```

This enables cloud-hypervisor's virtio-console as a pty,
and the guest starts a login prompt on `/dev/hvc0`.
Kernel and boot output still go to the serial port and the journal, so `logs` keeps working.
The trade-off: the initrd and emergency shells stay on the serial console, out of reach.

`microvmctl console <vm>` attaches, Ctrl-] detaches, and a second attach is refused
because two readers would each get half the output.
Press Enter after attaching to get a fresh prompt.

### SSH: full sessions over vsock

```nix
microvm.vsock.cid = 3; # unique per host, at least 3
microvm.vsock.ssh.enable = true;
users.users.root.openssh.authorizedKeys.keys = [ "ssh-ed25519 ..." ];
```

`microvmctl ssh root@<vm>` reaches the guest's sshd through cloud-hypervisor's vsock socket,
so it works even when the guest has no network.
microvmctl handles the vsock handshake itself,
so the host does not need `systemd-ssh-proxy` configured in `ssh_config`.
Host key checking is disabled for these connections:
only root on the host can open the socket,
and guests without a persistent `/etc` get new host keys on every boot.

## The TUI follows k9s conventions

| Key | Action |
|---|---|
| `j`/`k`, `g`/`G` | Move, jump to top or bottom |
| `/` | Filter by name (Esc clears) |
| `Enter` / `d` | Describe: the same output as `microvmctl status` |
| `l` | Follow logs (`j`/`k`, space/`b` to page, `G` or `f` to resume following) |
| `a` | Attach the console |
| `s` | SSH into the VM |
| `u` | Start |
| `D` / `R` | Stop / restart, after confirmation |
| `p` | Pause or resume |
| `Ctrl-r` | Refresh now |
| `?` | Help |
| `q` / Esc | Back or quit |

Every action runs the matching `microvmctl` command,
so the TUI behaves and fails exactly like the CLI.
Console and ssh take over the terminal and return to the table when they exit.

## Development

The flake provides a devshell with the Rust toolchain, rust-analyzer and nixfmt;
with direnv, `cd` into the repository and it loads from `.envrc`.

```console
$ nix develop
$ cargo test
$ nix flake check   # build, tests, clippy and rustfmt
```

### Test against real VMs without touching the host's config

cloud-hypervisor needs KVM, so test guests run on a microvm.nix host rather than locally.
The flake defines three small throwaway guests:
`mvmctl-test-a` (virtiofs store, a data volume, pty console and vsock SSH)
`mvmctl-test-b` (erofs store disk, deliberately without console or vsock)
and `mvmctl-test-c` (qemu, to exercise the cloud-hypervisor-only guard).
Neither has networking.

```console
$ test-vms up                                   # build, copy and install them on the host
$ test-vms ctl start mvmctl-test-a mvmctl-test-b mvmctl-test-c
$ test-vms ctl                                  # the TUI, freshly built, as root on the host
$ test-vms down                                 # stop them and remove their state
```

`test-vms` installs the guests as imperative VMs:
the host's existing `microvm@` template picks up
any `/var/lib/microvms/<name>/current/bin/microvm-run`.
That exercises the real units, socket permissions and journal
without changing the host's NixOS configuration, and the VMs never autostart.
Set `MVMCTL_TEST_HOST` to the host's ssh destination, e.g. `export MVMCTL_TEST_HOST=hypervisor-host`.
