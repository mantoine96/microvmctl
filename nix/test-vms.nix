# Throwaway microvm.nix guests for exercising microvmctl against real
# microvm@ units. They mirror the two storage layouts seen in practice:
# `a` shares the host store over virtiofs and has a data volume (like
# falcon), `b` boots from an erofs store disk (like osprey), `c` runs qemu
# instead of cloud-hypervisor. No network, so
# deploying them never touches the host's bridge or LAN.
{ nixpkgs, microvm }:

let
  mkTestVm =
    name: extraConfig:
    nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        microvm.nixosModules.microvm
        extraConfig
        {
          networking.hostName = name;
          system.stateVersion = "26.05";
          documentation.enable = false;
          microvm = {
            hypervisor = "cloud-hypervisor";
            vcpu = 1;
            mem = 512;
            interfaces = [ ];
          };
        }
      ];
    };
in
{
  mvmctl-test-a = mkTestVm "mvmctl-test-a" {
    microvm.shares = [
      {
        proto = "virtiofs";
        tag = "ro-store";
        source = "/nix/store";
        mountPoint = "/nix/.ro-store";
      }
    ];
    microvm.volumes = [
      {
        image = "data.img";
        mountPoint = "/data";
        size = 64;
      }
    ];

    # Interactive access for `microvmctl console` and `microvmctl ssh`;
    # `b` deliberately has neither, to exercise their error messages.
    microvm.cloud-hypervisor.extraArgs = [
      "--console"
      "pty"
    ];
    microvm.vsock = {
      cid = 4201;
      ssh.enable = true;
    };
    # NOTE: passwordless root is acceptable only because this guest has no
    # network: its console pty and vsock socket are reachable by host root only.
    users.users.root.hashedPassword = "";
    services.getty.autologinUser = "root";
    services.openssh.settings = {
      PermitRootLogin = "yes";
      PermitEmptyPasswords = "yes";
    };
    security.pam.services.sshd.allowNullPassword = true;
  };

  mvmctl-test-b = mkTestVm "mvmctl-test-b" { };

  # qemu exposes QMP at the same `<name>.sock` path as cloud-hypervisor's
  # API, which exercises microvmctl's hypervisor guard.
  mvmctl-test-c = mkTestVm "mvmctl-test-c" {
    microvm.hypervisor = nixpkgs.lib.mkForce "qemu";
  };
}
