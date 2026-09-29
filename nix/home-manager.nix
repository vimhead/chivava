{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.chivava;
  jsonFormat = pkgs.formats.json { };
  isName = value: builtins.isString value && value != "";
  isNames = value: builtins.isList value && lib.all isName value;
  validateFields =
    fields: validators:
    builtins.isAttrs fields
    && lib.all (name: builtins.hasAttr name validators && validators.${name} fields.${name}) (
      builtins.attrNames fields
    );
  validators = {
    mode =
      value:
      builtins.elem value [
        "prose"
        "code"
      ];
    seconds =
      value:
      builtins.elem value [
        null
        15
        30
        60
      ];
    theme = value: isName value || builtins.isPath value;
    sound =
      value:
      validateFields value {
        is_enabled = builtins.isBool;
        volume = level: builtins.isInt level && level >= 0 && level <= 100;
      };
    prose =
      value:
      validateFields value {
        capitals = builtins.isBool;
        punctuation = builtins.isBool;
        books = isNames;
      };
    code =
      value:
      validateFields value {
        languages = isNames;
      };
  };
in
{
  options.programs.chivava = {
    enable = lib.mkEnableOption "Chivava typing practice";
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "Chivava package to install.";
    };
    settings = lib.mkOption {
      type = jsonFormat.type;
      default = { };
      example = {
        mode = "code";
        seconds = null;
        theme = "catppuccin-latte";
        code.languages = [
          "Rust"
          "Python"
        ];
      };
      description = ''
        Explicitly managed settings written to chivava/managed.json under
        xdg.configHome. Only declared leaf settings become read-only in F2
        and config set. Undeclared preferences stay writable in config.json.
        An empty attribute set installs the package without managing settings.
        Use native JSON types: null disables the timer, booleans control toggles,
        and empty book/language lists select all sources. Sound fields are
        is_enabled and volume. Theme accepts a built-in name or a JSON file path.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = validateFields cfg.settings validators;
        message = "programs.chivava.settings contains an unknown key or invalid value; use native Chivava settings (mode, seconds, theme, sound.is_enabled/volume, prose.capitals/punctuation/books, code.languages).";
      }
    ];
    home.packages = [ cfg.package ];
    xdg.configFile."chivava/managed.json" = lib.mkIf (cfg.settings != { }) {
      source = jsonFormat.generate "chivava-managed.json" cfg.settings;
    };
  };
}
