#!/usr/bin/env python3
"""Run the actual POSIX installer against isolated release/download fixtures."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().parents[1] / "install.sh"
VERSION = "0.4.1"


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="hc-tui-installer-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.tools = self.root / "tools"
        self.assets = self.root / "assets"
        self.home = self.root / "home"
        for directory in [self.tools, self.assets, self.home]:
            directory.mkdir()
        self.env = dict(os.environ, HOME=str(self.home), PATH=f"{self.tools}:{os.environ['PATH']}",
                        MOCK_SYSTEM="Linux", MOCK_ARCH="x86_64", MOCK_ROSETTA="0",
                        MOCK_ASSETS=str(self.assets), MOCK_LOG=str(self.root / "downloads"))
        self.env.pop("HC_TUI_INSTALL_DIR", None)
        self.tool("uname", '#!/bin/sh\ncase "$1" in -s) echo "$MOCK_SYSTEM";; -m) echo "$MOCK_ARCH";; esac\n')
        self.tool("sysctl", '#!/bin/sh\necho "$MOCK_ROSETTA"\n')
        self.tool("curl", f"#!{sys.executable}\n" + '''import os, pathlib, shutil, sys
args=sys.argv[1:]
url=args[-1]
with open(os.environ['MOCK_LOG'], 'a') as log: log.write(url+'\\n')
if os.environ.get('MOCK_FAIL') == '1': sys.exit(22)
if '--head' in args:
    print('https://github.com/wuxiaobai24/hc-tui/releases/tag/v0.4.1', end='')
else:
    source=pathlib.Path(os.environ['MOCK_ASSETS'])/url.rsplit('/',1)[-1]
    if not source.is_file(): sys.exit(22)
    shutil.copyfile(source,args[args.index('--output')+1])
''')

    def tool(self, name, content):
        path = self.tools / name
        path.write_text(content)
        path.chmod(0o755)

    def archive(self, target, *, corrupt=False, binary_version=VERSION, symlink=False):
        name = f"hc-tui-{VERSION}-{target}.tar.gz"
        path = self.assets / name
        executable = f"#!/bin/sh\nprintf 'hc-tui {binary_version}\\n'\n".encode()
        with tarfile.open(path, "w:gz") as archive:
            member = tarfile.TarInfo("./hc-tui")
            member.mode = 0o755
            if symlink:
                member.type = tarfile.SYMTYPE
                member.linkname = "/outside"
                archive.addfile(member)
            else:
                member.size = len(executable)
                archive.addfile(member, io.BytesIO(executable))
        digest = "0" * 64 if corrupt else hashlib.sha256(path.read_bytes()).hexdigest()
        (self.assets / (name + ".sha256")).write_text(f"{digest}  {name}\n")
        return executable

    def install(self, *args, script=None):
        return subprocess.run(["sh", "-s", "--", *args], input=script or INSTALLER.read_text(),
                              env=self.env, text=True, capture_output=True)

    def test_all_platforms_and_rosetta_choose_the_right_archive(self):
        for system, arch, rosetta, target in [
            ("Linux", "x86_64", "0", "x86_64-unknown-linux-musl"),
            ("Linux", "aarch64", "0", "aarch64-unknown-linux-musl"),
            ("Darwin", "x86_64", "0", "x86_64-apple-darwin"),
            ("Darwin", "arm64", "1", "aarch64-apple-darwin"),
            ("Darwin", "x86_64", "1", "aarch64-apple-darwin"),
        ]:
            with self.subTest(system=system, arch=arch, rosetta=rosetta):
                self.env.update(MOCK_SYSTEM=system, MOCK_ARCH=arch, MOCK_ROSETTA=rosetta)
                expected = self.archive(target)
                result = self.install()
                self.assertEqual(result.returncode, 0, result.stderr)
                binary = self.home / ".local/bin/hc-tui"
                self.assertEqual(binary.read_bytes(), expected)
                self.assertEqual(binary.stat().st_mode & 0o777, 0o755)
                self.assertIn(target, result.stdout)
                self.assertIn("hc-tui update", result.stdout)
                self.assertFalse(list(binary.parent.glob(".hc-tui-install.*")))

    def test_explicit_version_and_directory_with_spaces(self):
        expected = self.archive("x86_64-unknown-linux-musl")
        directory = self.root / "chosen bin directory"
        result = self.install("--version", f"v{VERSION}", "--bin-dir", str(directory))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((directory / "hc-tui").read_bytes(), expected)
        self.assertNotIn("/releases/latest", (self.root / "downloads").read_text())

    def test_environment_directory(self):
        self.archive("x86_64-unknown-linux-musl")
        self.env["HC_TUI_INSTALL_DIR"] = str(self.root / "env-bin")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.root / "env-bin/hc-tui").is_file())

    def test_failed_download_checksum_version_or_symlink_preserves_old_binary(self):
        directory = self.root / "existing"
        directory.mkdir()
        binary = directory / "hc-tui"
        binary.write_bytes(b"old installation")
        for failure in ["download", "checksum", "version", "symlink"]:
            with self.subTest(failure=failure):
                self.env["MOCK_FAIL"] = "1" if failure == "download" else "0"
                self.archive("x86_64-unknown-linux-musl", corrupt=failure == "checksum",
                             binary_version="9.9.9" if failure == "version" else VERSION,
                             symlink=failure == "symlink")
                result = self.install("--version", VERSION, "--bin-dir", str(directory))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn({"download":"download failed", "checksum":"SHA-256 mismatch", "version":"version does not match", "symlink":"not a regular file"}[failure], result.stderr)
                self.assertEqual(binary.read_bytes(), b"old installation")
                self.assertFalse(list(directory.glob(".hc-tui-install.*")))

    def test_invalid_platform_or_version_does_not_install(self):
        for value in ["../other", "1.2", "0.4.1\n../other", ""]:
            with self.subTest(version=value):
                result = self.install("--version", value)
                self.assertNotEqual(result.returncode, 0)
        self.env["MOCK_SYSTEM"] = "Windows_NT"
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.home / ".local/bin/hc-tui").exists())

    def test_truncated_piped_script_has_no_installation_side_effects(self):
        text = INSTALLER.read_text()
        result = self.install(script=text[:len(text) // 2])
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "downloads").exists())
        self.assertFalse((self.home / ".local/bin/hc-tui").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
