# Building and testing

Use a current stable Rust toolchain. Linux also needs a C compiler, `pkg-config`, and ALSA development headers (`build-essential pkg-config libasound2-dev` on Ubuntu).

```sh
cargo install --path . --locked
```

Run checks from the repository root:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s tests -p 'test_*.py' -v
```

The hardware audio test is ignored by default. Installer tests use mocked downloads and do not modify your installation.

For Nix changes:

```sh
nix flake check
nix flake check --all-systems --no-build
nix fmt -- --check flake.nix nix/*.nix
```
