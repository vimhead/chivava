{
  pkgs,
  home-manager,
  package,
  module,
}:
let
  inherit (pkgs) lib;
  evaluate =
    extra:
    (home-manager.lib.homeManagerConfiguration {
      inherit pkgs;
      modules = [
        module
        {
          home = {
            username = "chivava-test";
            homeDirectory = "/tmp/chivava-test";
            stateVersion = "25.05";
          };
          programs.chivava = {
            enable = true;
            inherit package;
          };
        }
        extra
      ];
    }).config;
  unmanaged = evaluate { };
  disabled = evaluate { programs.chivava.enable = lib.mkForce false; };
  settings = {
    mode = "code";
    seconds = null;
    theme = "nord";
    sound.is_enabled = false;
    code.languages = [
      "Rust"
      "Python"
    ];
  };
  managed = evaluate { programs.chivava.settings = settings; };
  managedFile = managed.xdg.configFile."chivava/managed.json".source;
  hasManagedFile =
    configuration: builtins.hasAttr "chivava/managed.json" configuration.xdg.configFile;
  hasWritableFile =
    configuration: builtins.hasAttr "chivava/config.json" configuration.xdg.configFile;
  isValid = configuration: lib.all (item: item.assertion) configuration.assertions;
  invalidSettings = [
    { typo = true; }
    { seconds = 17; }
    { mode = "invalid"; }
    { sound.volume = 101; }
    { sound.enabled = false; }
    { prose.capitals = "off"; }
    { code.languages = "Rust"; }
  ];
in
assert isValid unmanaged;
assert isValid managed;
assert builtins.elem package unmanaged.home.packages;
assert !(builtins.elem package disabled.home.packages);
assert !(hasManagedFile unmanaged);
assert !(hasManagedFile disabled);
assert !(hasWritableFile unmanaged);
assert !(hasWritableFile managed);
assert hasManagedFile managed;
assert lib.all (
  invalid:
  !(builtins.tryEval (
    isValid (evaluate {
      programs.chivava.settings = invalid;
    })
  )).success
) invalidSettings;
pkgs.runCommand "chivava-home-manager-check" { nativeBuildInputs = [ pkgs.python3 ]; } ''
  export CHIVAVA_CONFIG_DIR="$TMPDIR/preferences"
  mkdir -p "$CHIVAVA_CONFIG_DIR"
  ln -s ${managedFile} "$CHIVAVA_CONFIG_DIR/managed.json"
  export CHIVAVA_BINARY=${package}/bin/chivava
  python3 ${./check-module.py}
  touch "$out"
''
