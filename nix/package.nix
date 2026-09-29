{
  lib,
  rustPlatform,
  stdenv,
  pkg-config,
  alsa-lib,
}:
let
  manifest = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = "chivava";
  version = manifest.package.version;
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../LICENSE
      ../src
      ../assets
      ../examples
      ../tests
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;
  nativeBuildInputs = [ pkg-config ];
  buildInputs = lib.optionals stdenv.hostPlatform.isLinux [ alsa-lib ];
  env.CHIVAVA_BUILD_VERSION = "${manifest.package.version}-nix";

  meta = {
    description = manifest.package.description;
    homepage = "https://github.com/vimhead/chivava";
    license = lib.licenses.mit;
    mainProgram = "chivava";
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
  };
}
