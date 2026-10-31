{
  description = "microvmctl: manage and observe microvm.nix cloud-hypervisor VMs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # NOTE: only used for the test guests (`test-vms`); the package does not depend on it.
    microvm = {
      url = "github:microvm-nix/microvm.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      microvm,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (
        pkgs:
        {
          default = self.packages.${pkgs.stdenv.hostPlatform.system}.microvmctl;
          microvmctl = pkgs.callPackage ./package.nix { };
        }
        // nixpkgs.lib.optionalAttrs (pkgs.stdenv.hostPlatform.system == "x86_64-linux") (
          builtins.mapAttrs (_: vm: vm.config.microvm.declaredRunner) self.nixosConfigurations
        )
      );

      overlays.default = final: _prev: {
        microvmctl = final.callPackage ./package.nix { };
      };

      nixosConfigurations = import ./nix/test-vms.nix { inherit nixpkgs microvm; };

      devShells = forAllSystems (pkgs: {
        # NOTE: no `inputsFrom` the package: it needs Cargo.lock to evaluate,
        # which would make the shell unusable for generating it.
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            clippy
            rust-analyzer
            rustc
            rustfmt
            nixfmt
            (pkgs.callPackage ./nix/test-vms-script.nix { })
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      checks = forAllSystems (pkgs: {
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.microvmctl;
        clippy = self.packages.${pkgs.stdenv.hostPlatform.system}.microvmctl.overrideAttrs (old: {
          pname = "${old.pname}-clippy";
          nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
          buildPhase = "cargo clippy --offline --all-targets -- --deny warnings";
          installPhase = "touch $out";
          doCheck = false;
        });
        fmt =
          pkgs.runCommand "microvmctl-fmt"
            {
              nativeBuildInputs = [
                pkgs.cargo
                pkgs.rustfmt
              ];
            }
            ''
              cd ${./.}
              cargo fmt --check
              touch $out
            '';
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
