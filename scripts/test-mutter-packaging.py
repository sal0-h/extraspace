#!/usr/bin/env python3
"""Check installer boundaries with fake package tools; never call real sudo."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="extraspace-packaging-")
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.tools = self.directory / "bin"
        self.tools.mkdir()
        self.calls = self.directory / "calls"
        self.calls.touch()
        self.installed = self.directory / "installed"
        self.installed.write_text("51.1-1")
        self.package = self.directory / "mutter-51.1-2-x86_64.pkg.tar.zst"
        self.script = self.directory / "build-and-install.sh"
        shutil.copyfile(ROOT / "packaging/mutter-patch/build-and-install.sh", self.script)
        (self.directory / "PKGBUILD").write_text("pkgver=51.1\npkgrel=2\n")
        self.environment = os.environ.copy()
        self.environment.update(
            PATH=f"{self.tools}:{os.environ['PATH']}",
            XS_TEST_DIRECTORY=str(self.directory),
            XS_TEST_PACKAGE_VERSION="51.1-2",
            XS_TEST_VERSION_ORDER="1",
            XS_TEST_BUILD_STATUS="0",
            XS_TEST_SUDO_STATUS="0",
        )
        self.tool("pacman", '''
case "$1" in
  -Q) printf 'mutter %s\\n' "$(cat "$XS_TEST_DIRECTORY/installed")" ;;
  -Qp) printf 'mutter %s\\n' "$XS_TEST_PACKAGE_VERSION" ;;
  *) exit 99 ;;
esac
''')
        self.tool("vercmp", '''
[ "$1" = 51.1-2 ] || exit 99
[ "$2" = "$(cat "$XS_TEST_DIRECTORY/installed")" ] || exit 99
if [ "$2" = 51.2-1 ]; then echo -1; else echo "$XS_TEST_VERSION_ORDER"; fi
''')
        self.tool("makepkg", '''
if [ -n "${XS_TEST_UPGRADE_DURING_BUILD:-}" ]; then
  echo 51.2-1 > "$XS_TEST_DIRECTORY/installed"
fi
touch "$XS_TEST_DIRECTORY/mutter-51.1-2-x86_64.pkg.tar.zst"
exit "$XS_TEST_BUILD_STATUS"
''')
        self.tool("sudo", 'exit "$XS_TEST_SUDO_STATUS"\n')
        self.tool("install-hook.sh", "", self.directory)

    def tool(self, name, body, directory=None):
        path = (directory or self.tools) / name
        path.write_text(
            '#!/bin/sh\nset -eu\n'
            f'printf "%s\\n" "{name} $*" >> "$XS_TEST_DIRECTORY/calls"\n'
            + body
        )
        path.chmod(0o755)

    def run_installer(self, *arguments):
        return subprocess.run(
            ["sh", str(self.script), *arguments],
            env=self.environment, capture_output=True, text=True, check=False,
        )

    def assert_no_install(self):
        calls = self.calls.read_text()
        self.assertNotIn("sudo pacman", calls)
        self.assertNotIn("install-hook.sh", calls)

    def test_old_recipe_refuses_downgrade_before_build(self):
        self.installed.write_text("51.2-1")
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("refusing to downgrade", result.stderr)
        self.assertNotIn("makepkg", self.calls.read_text())
        self.assert_no_install()

    def test_install_only_requires_a_package(self):
        result = self.run_installer("--install-only")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Package not found", result.stderr)
        self.assertNotIn("sudo", self.calls.read_text())

    def test_wrong_package_version_never_reaches_sudo(self):
        self.package.touch()
        self.environment["XS_TEST_PACKAGE_VERSION"] = "51.0-2"
        result = self.run_installer("--install-only")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("metadata does not match", result.stderr)
        self.assertNotIn("sudo", self.calls.read_text())

    def test_build_only_does_not_install(self):
        result = self.run_installer("--build-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("makepkg -sf", self.calls.read_text())
        self.assertNotIn("sudo", self.calls.read_text())

    def test_install_only_skips_build_and_records_after_install(self):
        self.package.touch()
        result = self.run_installer("--install-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls.read_text()
        self.assertNotIn("makepkg", calls)
        self.assertLess(calls.index("sudo pacman -U"), calls.index("install-hook.sh"))

    def test_failed_build_never_installs_existing_package(self):
        self.package.touch()
        self.environment["XS_TEST_BUILD_STATUS"] = "1"
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assert_no_install()

    def test_upgrade_during_build_prevents_install(self):
        self.environment["XS_TEST_UPGRADE_DURING_BUILD"] = "1"
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("refusing to downgrade", result.stderr)
        self.assert_no_install()

    def test_failed_authentication_does_not_record_patch(self):
        self.package.touch()
        self.environment["XS_TEST_SUDO_STATUS"] = "1"
        self.assertNotEqual(self.run_installer("--install-only").returncode, 0)
        self.assert_no_install()

    def test_invalid_arguments_do_not_build(self):
        for arguments in [("--typo",), ("--build-only", "unexpected")]:
            with self.subTest(arguments=arguments):
                self.assertNotEqual(self.run_installer(*arguments).returncode, 0)
                self.assertEqual(self.calls.read_text(), "")


if __name__ == "__main__":
    unittest.main()
