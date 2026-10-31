{ writeShellApplication, openssh }:

writeShellApplication {
  name = "test-vms";
  runtimeInputs = [ openssh ];
  # Remote commands are built locally on purpose: store paths and VM names
  # are known client-side and contain no shell metacharacters.
  excludeShellChecks = [ "SC2029" ];
  text = ''
    # Deploys the flake's test guests to a microvm.nix host as imperative
    # VMs: the host's own microvm@ template picks up any
    # /var/lib/microvms/<name>/current/bin/microvm-run, so nothing in the
    # host's NixOS config changes and the VMs never autostart.
    host="''${MVMCTL_TEST_HOST:-10.100.10.18}"
    state_dir=/var/lib/microvms
    vms=(mvmctl-test-a mvmctl-test-b mvmctl-test-c)
    flake="$(git rev-parse --show-toplevel)"

    build_and_copy() {
      local out
      out="$(nix build --no-link --print-out-paths "$flake#$1")"
      nix copy --substitute-on-destination --to "ssh://$host" "$out"
      echo "$out"
    }

    usage() {
      cat <<EOF
    Usage: test-vms <command>

      up          build the test VMs and install them on $host (not started)
      down        stop the test VMs and remove their state directories
      ctl [ARGS]  run the freshly built microvmctl on $host as root (no ARGS: TUI)
      list        names of the test VMs

    Set MVMCTL_TEST_HOST to target another host.
    EOF
    }

    case "''${1:-}" in
      up)
        for vm in "''${vms[@]}"; do
          echo "==> $vm"
          runner="$(build_and_copy "$vm")"
          ssh "$host" "sudo install --directory --owner=microvm --group=kvm --mode=775 $state_dir/$vm \
            && sudo ln --symbolic --force --no-dereference $runner $state_dir/$vm/current \
            && sudo chown --no-dereference microvm:kvm $state_dir/$vm/current"
        done
        echo "Installed. Try: test-vms ctl start ''${vms[*]}"
        ;;
      down)
        units=()
        for vm in "''${vms[@]}"; do units+=("microvm@$vm.service"); done
        # NOTE: stop failures are ignored so a half-deployed state can still be cleaned.
        ssh "$host" "sudo systemctl stop ''${units[*]} || true; sudo systemctl reset-failed ''${units[*]} 2>/dev/null || true"
        for vm in "''${vms[@]}"; do
          ssh "$host" "sudo rm --recursive --force -- $state_dir/$vm"
        done
        echo "Removed ''${vms[*]} from $host."
        ;;
      ctl)
        shift
        ctl="$(build_and_copy microvmctl)"
        # ssh joins arguments into one remote shell line; re-quote them so
        # JSON bodies and the like survive. With no arguments, printf would
        # still emit an empty quoted word, turning "open the TUI" into an
        # empty subcommand.
        args=""
        if (($# > 0)); then args="$(printf '%q ' "$@")"; fi
        ssh -t "$host" sudo "$ctl/bin/microvmctl" "$args"
        ;;
      list)
        printf '%s\n' "''${vms[@]}"
        ;;
      *)
        usage
        exit 1
        ;;
    esac
  '';
}
