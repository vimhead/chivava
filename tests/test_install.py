import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest


INSTALLER = Path(__file__).resolve().parents[1] / "scripts/install.sh"
ASSETS = [
    "chivava-linux-x64",
    "chivava-linux-arm64",
    "chivava-darwin-x64",
    "chivava-darwin-arm64",
    "chivava-windows-x64.exe",
]
BINARY = b'#!/bin/sh\nprintf executed > "$EXECUTION_MARKER"\nprintf "chivava 0.1.0-tip.test\\n"\n'


@unittest.skipUnless(os.name == "posix", "installer harness requires a POSIX shell")
class InstallTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="chivava install test ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.tools = self.root / "tools"
        self.release = self.root / "release"
        self.scratch = self.root / "scratch"
        self.destination = self.root / "installed binaries"
        self.marker = self.root / "executed"
        for directory in [self.tools, self.release, self.scratch]:
            directory.mkdir()
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("CHIVAVA_")}
        self.env.update({
            "PATH": str(self.tools),
            "HOME": str(self.root / "home"),
            "TMPDIR": str(self.scratch),
            "CHIVAVA_INSTALL_DIR": str(self.destination),
            "MOCK_OS": "Linux",
            "MOCK_MACHINE": "x86_64",
            "MOCK_TRANSLATED": "0",
            "MOCK_RELEASE": str(self.release),
            "MOCK_LOG": str(self.root / "downloads"),
            "EXECUTION_MARKER": str(self.marker),
        })
        for utility in ["awk", "tr", "mktemp", "mkdir", "rm", "cp", "chmod", "mv"]:
            self.link_tool(utility)
        checksum = "sha256sum" if shutil.which("sha256sum") else "shasum"
        self.link_tool(checksum)
        self.make_tool("uname", 'import os, sys\nprint(os.environ["MOCK_OS" if sys.argv[1] == "-s" else "MOCK_MACHINE"])\n')
        self.make_tool("sysctl", 'import os\nprint(os.environ["MOCK_TRANSLATED"])\n')
        self.make_tool("curl", self.download_program())
        for asset in ASSETS:
            self.write_asset(asset, BINARY)

    def link_tool(self, name):
        executable = shutil.which(name)
        self.assertIsNotNone(executable, name)
        (self.tools / name).symlink_to(executable)

    def make_tool(self, name, program):
        path = self.tools / name
        path.write_text(f"#!{sys.executable}\n" + program)
        path.chmod(0o755)

    def download_program(self):
        return '''import os
from pathlib import Path
import shutil
import sys
from urllib.parse import urlparse

arguments = sys.argv[1:]
if Path(sys.argv[0]).name == "curl":
    assert arguments[:3] == ["--proto", "=https", "--tlsv1.2"], arguments
    url, output = arguments[-3], arguments[-1]
else:
    assert arguments[:2] == ["--https-only", "-qO"], arguments
    output, url = arguments[-2:]
with open(os.environ["MOCK_LOG"], "a") as log:
    log.write(url + "\\n")
asset = Path(urlparse(url).path).name
if os.environ.get("FAIL_DOWNLOAD") == asset:
    Path(output).write_bytes(b"partial download")
    sys.exit(22)
source = Path(os.environ["MOCK_RELEASE"]) / asset
if not source.is_file():
    sys.exit(22)
shutil.copyfile(source, output)
'''

    def write_asset(self, asset, content):
        (self.release / asset).write_bytes(content)
        checksum = hashlib.sha256(content).hexdigest()
        (self.release / (asset + ".sha256")).write_text(f"{checksum}  {asset}\n")

    def run_installer(self, arguments=()):
        result = subprocess.run(["/bin/sh", str(INSTALLER), *arguments], env=self.env, capture_output=True, text=True)
        self.assertEqual(list(self.scratch.iterdir()), [], result.stderr)
        if self.destination.exists():
            self.assertEqual(list(self.destination.glob(".chivava-install.*")), [], result.stderr)
        return result

    def assert_installed(self, asset):
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        name = "chivava.exe" if asset.endswith(".exe") else "chivava"
        installed = self.destination / name
        self.assertEqual(installed.read_bytes(), BINARY)
        self.assertEqual(stat.S_IMODE(installed.stat().st_mode), 0o755)
        self.assertTrue(self.marker.exists())
        urls = Path(self.env["MOCK_LOG"]).read_text().splitlines()
        self.assertEqual(urls[-2:], [f"https://github.com/vimhead/chivava/releases/download/tip/{asset}",
                                     f"https://github.com/vimhead/chivava/releases/download/tip/{asset}.sha256"])
        self.assertIn(str(installed), result.stdout)

    def test_supported_platforms_and_architecture_aliases(self):
        for system, machine, asset in [
            ("Linux", "x86_64", "chivava-linux-x64"),
            ("Linux", "amd64", "chivava-linux-x64"),
            ("Linux", "aarch64", "chivava-linux-arm64"),
            ("Linux", "arm64", "chivava-linux-arm64"),
            ("Darwin", "x86_64", "chivava-darwin-x64"),
            ("Darwin", "arm64", "chivava-darwin-arm64"),
            ("MINGW64_NT-10.0", "x86_64", "chivava-windows-x64.exe"),
            ("MSYS_NT-10.0", "x86_64", "chivava-windows-x64.exe"),
            ("CYGWIN_NT-10.0", "x86_64", "chivava-windows-x64.exe"),
        ]:
            with self.subTest(system=system, machine=machine):
                self.env.update(MOCK_OS=system, MOCK_MACHINE=machine)
                self.assert_installed(asset)

    def test_rosetta_selects_native_apple_silicon(self):
        self.env.update(MOCK_OS="Darwin", MOCK_MACHINE="x86_64", MOCK_TRANSLATED="1")
        self.assert_installed("chivava-darwin-arm64")

    def test_unsupported_systems_fail_before_downloading(self):
        for system, machine in [("FreeBSD", "amd64"), ("Linux", "i686"), ("Linux", "armv7l"), ("MINGW64_NT", "arm64")]:
            with self.subTest(system=system, machine=machine):
                self.env.update(MOCK_OS=system, MOCK_MACHINE=machine)
                self.assertNotEqual(self.run_installer().returncode, 0)
                self.assertFalse(Path(self.env["MOCK_LOG"]).exists())
                self.assertFalse(self.destination.exists())

    def test_default_destination_and_path_hint(self):
        self.env.pop("CHIVAVA_INSTALL_DIR")
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        destination = Path(self.env["HOME"]) / ".local/bin/chivava"
        self.assertEqual(destination.read_bytes(), BINARY)
        self.assertIn("Add", result.stdout)
        self.env["PATH"] += ":" + str(destination.parent)
        self.assertNotIn("Add", self.run_installer().stdout)

    def test_explicit_destination_does_not_require_home(self):
        self.env.pop("HOME")
        self.assert_installed("chivava-linux-x64")
        self.env.pop("CHIVAVA_INSTALL_DIR")
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("HOME is required", result.stderr)

    def test_repository_tag_and_https_mirror_overrides(self):
        self.env.update(CHIVAVA_REPOSITORY="owner/fork", CHIVAVA_RELEASE_TAG="custom")
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        urls = Path(self.env["MOCK_LOG"]).read_text()
        self.assertIn("https://github.com/owner/fork/releases/download/custom/chivava-linux-x64", urls)
        self.env["CHIVAVA_RELEASE_BASE_URL"] = "https://mirror.example/releases/build"
        self.assertEqual(self.run_installer().returncode, 0)
        self.assertIn("https://mirror.example/releases/build/chivava-linux-x64", Path(self.env["MOCK_LOG"]).read_text())
        self.env["CHIVAVA_RELEASE_BASE_URL"] = "http://insecure.example"
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("HTTPS", result.stderr)

    def test_wget_fallback(self):
        (self.tools / "curl").unlink()
        self.make_tool("wget", self.download_program())
        self.assert_installed("chivava-linux-x64")

    @unittest.skipUnless(shutil.which("shasum"), "shasum is unavailable")
    def test_shasum_fallback(self):
        checksum = self.tools / "sha256sum"
        if checksum.exists():
            checksum.unlink()
            self.link_tool("shasum")
        self.assert_installed("chivava-linux-x64")

    def test_missing_downloader_or_checksum_tool(self):
        (self.tools / "curl").unlink()
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("curl or wget", result.stderr)
        self.make_tool("curl", self.download_program())
        for name in ["sha256sum", "shasum"]:
            (self.tools / name).unlink(missing_ok=True)
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("sha256sum or shasum", result.stderr)
        self.assertFalse(self.destination.exists())
        self.assertFalse(self.marker.exists())

    def test_malformed_checksums_never_execute_or_replace_binary(self):
        self.destination.mkdir()
        target = self.destination / "chivava"
        target.write_bytes(b"existing binary")
        for checksum in ["", "1234\n", "g" * 64, "0" * 64, "a" * 64 + "\nextra line\n"]:
            with self.subTest(checksum=checksum):
                (self.release / "chivava-linux-x64.sha256").write_text(checksum)
                result = self.run_installer()
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.marker.exists())
                self.assertEqual(target.read_bytes(), b"existing binary")

    def test_uppercase_checksum_is_accepted(self):
        checksum = self.release / "chivava-linux-x64.sha256"
        checksum.write_text(checksum.read_text().upper())
        self.assert_installed("chivava-linux-x64")

    def test_download_failures_preserve_existing_installation(self):
        self.destination.mkdir()
        target = self.destination / "chivava"
        target.write_bytes(b"existing binary")
        for asset in ["chivava-linux-x64", "chivava-linux-x64.sha256"]:
            with self.subTest(asset=asset):
                self.env["FAIL_DOWNLOAD"] = asset
                result = self.run_installer()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(target.read_bytes(), b"existing binary")
                self.assertFalse(self.marker.exists())

    def test_unrunnable_or_unexpected_binary_is_not_installed(self):
        self.destination.mkdir()
        target = self.destination / "chivava"
        target.write_bytes(b"existing binary")
        for content in [b"#!/bin/sh\nexit 1\n", b"#!/bin/sh\nprintf 'unexpected program'\n"]:
            with self.subTest(content=content):
                self.write_asset("chivava-linux-x64", content)
                self.assertNotEqual(self.run_installer().returncode, 0)
                self.assertEqual(target.read_bytes(), b"existing binary")

    def test_upgrade_replaces_a_symlink_not_its_target(self):
        self.destination.mkdir()
        other = self.root / "other executable"
        other.write_bytes(b"untouched")
        target = self.destination / "chivava"
        target.symlink_to(other)
        self.assert_installed("chivava-linux-x64")
        self.assertFalse(target.is_symlink())
        self.assertEqual(other.read_bytes(), b"untouched")

    def test_directory_at_target_is_rejected(self):
        target = self.destination / "chivava"
        target.mkdir(parents=True)
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("is a directory", result.stderr)
        self.assertEqual(list(target.iterdir()), [])
        self.assertFalse(Path(self.env["MOCK_LOG"]).exists())

    def test_failed_rename_cleans_staging_file_and_preserves_old_binary(self):
        self.destination.mkdir()
        target = self.destination / "chivava"
        target.write_bytes(b"existing binary")
        (self.tools / "mv").unlink()
        self.make_tool("mv", "import sys\nsys.exit(1)\n")
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertEqual(target.read_bytes(), b"existing binary")

    def test_installer_can_run_from_standard_input(self):
        result = subprocess.run(["/bin/sh"], input=INSTALLER.read_text(), env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.destination / "chivava").read_bytes(), BINARY)
        self.assertEqual(list(self.scratch.iterdir()), [])

    def test_unexpected_arguments_are_rejected(self):
        result = self.run_installer(["--unknown"])
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(Path(self.env["MOCK_LOG"]).exists())


if __name__ == "__main__":
    unittest.main()
