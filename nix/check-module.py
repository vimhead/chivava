import json
import os
from pathlib import Path
import subprocess


binary = os.environ["CHIVAVA_BINARY"]
directory = Path(os.environ["CHIVAVA_CONFIG_DIR"])
managed_path = directory / "managed.json"
managed_bytes = managed_path.read_bytes()
assert json.loads(managed_bytes) == {
    "mode": "code",
    "seconds": None,
    "theme": "nord",
    "sound": {"is_enabled": False},
    "code": {"languages": ["Rust", "Python"]},
}
assert not (directory / "config.json").exists()


def run(*arguments):
    return subprocess.check_output([binary, *arguments], text=True)


listed = run("config", "list")
assert "mode = code [Nix]" in listed
assert "seconds = off [Nix]" in listed
assert "sound = off [Nix]" in listed
assert "code.languages = Rust,Python [Nix]" in listed
for key, value in [("theme", "catppuccin-mocha"), ("seconds", "30"), ("sound", "on")]:
    result = subprocess.run([binary, "config", "set", key, value], capture_output=True, text=True)
    assert result.returncode != 0
    assert "managed by Nix" in result.stderr
run("config", "set", "volume", "37")
run("config", "set", "prose.punctuation", "off")
stored = json.loads((directory / "config.json").read_text())["settings"]
assert stored["mode"] == "prose"
assert stored["theme"] == "catppuccin-mocha"
assert stored["seconds"] == 30
assert stored["sound"] == {"is_enabled": True, "volume": 37}
assert stored["prose"]["punctuation"] is False
assert managed_path.is_symlink()
assert managed_path.read_bytes() == managed_bytes
managed_path.unlink()
listed = run("config", "list")
assert "[Nix]" not in listed
assert "seconds = 30" in listed
assert "volume = 37" in listed
assert "prose.punctuation = off" in listed
