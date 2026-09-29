# Nix setup

## Home Manager

Add Chivava to your flake's `inputs`:

```nix
chivava = {
  url = "github:vimhead/chivava/master";
  inputs.nixpkgs.follows = "nixpkgs";
  inputs.home-manager.follows = "home-manager";
};
```

Pass `inputs` through Home Manager's `extraSpecialArgs`, then import the module:

```nix
{ inputs, ... }:
{
  imports = [ inputs.chivava.homeManagerModules.default ];
  programs.chivava.enable = true;
}
```

Enabling the module alone installs Chivava without managing preferences. Add declarations under `programs.chivava.settings` to manage individual settings:

| Setting | Values |
| --- | --- |
| `mode` | `"prose"`, `"code"` |
| `seconds` | `null` (off), `15`, `30`, `60` |
| `theme` | Built-in name or native theme JSON file path |
| `sound.is_enabled` | Boolean |
| `sound.volume` | Integer, 0–100 |
| `prose.capitals`, `prose.punctuation` | Boolean |
| `prose.books`, `code.languages` | Lists of names; `[]` selects all |

Use `chivava sources books`, `chivava sources languages`, and `chivava themes list` to discover names.

Restart Chivava after rebuilding. Removing a declaration restores the underlying personal/default value. Managing one audio field leaves the other independently editable in F2.

Home Manager places its declarations in `$XDG_CONFIG_HOME/chivava/managed.json`, leaving personal preferences in `config.json`. `CHIVAVA_CONFIG_DIR` selects a different directory for both files; it does not inherit declarations from the default directory.

`programs.chivava.package` can override the installed package. If an older Cargo/curl installation takes precedence, check `command -v chivava`; Nix builds report a `-nix` version suffix.

## Without Home Manager

```sh
nix profile add github:vimhead/chivava/master
```

## Platform compatibility

Linux and macOS are supported on ARM64 and x86-64. The standalone flake pins Nixpkgs/Home Manager 26.05 to retain Intel macOS support. For Intel macOS, keep that pin or follow compatible 26.05 inputs instead of the newer inputs in the example above; Nixpkgs 26.11 dropped this target.
