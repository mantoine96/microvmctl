# Throwaway microvm.nix guests for exercising microvmctl against real
# microvm@ units. They mirror the two storage layouts seen in practice:
# `a` shares the host store over virtiofs and has a data volume (like
# falcon), `b` boots from an erofs store disk (like osprey). No network, so
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

    # Interactive access for `microvmctl console`;
    # `b` deliberately lacks it, to exercise the error message.
    microvm.cloud-hypervisor.extraArgs = [
      "--console"
      "pty"
    ];
    # NOTE: passwordless root is acceptable only because this guest has no
    # network: its console pty is reachable by host root only.
    users.users.root.hashedPassword = "";
    services.getty.autologinUser = "root";
  };

  mvmctl-test-b = mkTestVm "mvmctl-test-b" { };
}
