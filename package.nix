{ lib, rustPlatform }:

let
  cargoToml = lib.importTOML ./Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  inherit (cargoToml.package) version;

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  # NOTE: systemctl and journalctl are deliberately not wrapped into PATH.
  # They must match the host's running systemd, so we use whatever the
  # host system provides.

  meta = {
    description = cargoToml.package.description;
    license = lib.licenses.mit;
    mainProgram = "microvmctl";
    platforms = lib.platforms.linux;
  };
}
