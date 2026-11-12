"""python3 -m unittest discover -s scripts -p 'test_*.py'"""
import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

TOOL = Path(__file__).resolve().parents[1] / "native" / "tools" / "native.py"
REVISION = "0123456789abcdef0123456789abcdef01234567"


def load_native():
    spec = importlib.util.spec_from_file_location("native_tool", TOOL)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def make_mod(root):
    """native/tiny/ with one bundle: a helper, a data folder and the mod's LICENSE."""
    mod = root / "tiny"
    bundle = mod / "mixins" / "tiny"
    (bundle / "data").mkdir(parents=True)
    (mod / "compatibility.json").write_text(json.dumps({
        "game": "26.3", "horizon_revision": REVISION, "horizon_mixins_revision": REVISION, "hooks": 1}))
    (bundle / "horizon-mixin.toml").write_text(
        f'[mod]\nid = "tiny"\nversion = "1.0.0"\n\n[compatibility]\nhorizon_revision = "{REVISION}"\n')
    (bundle / "helper.rs").write_text("pub fn helper() {}\n")
    (bundle / "data" / "table.json").write_text("{}\n")
    (mod / "LICENSE").write_text("MIT\n")


class PackageTest(unittest.TestCase):
    def package_bytes(self, platform):
        native = load_native()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            make_mod(root)
            with mock.patch.object(native, "NATIVE", root), mock.patch.object(native, "TARGET", root / "target"):
                # zipfile writes the creating system into every header, from sys.platform.
                with mock.patch.object(sys, "platform", platform), contextlib.redirect_stdout(io.StringIO()):
                    native.package(native.Mod("tiny"))
                return (root / "target" / "tiny-1.0.0.hnmod").read_bytes()

    def test_the_package_does_not_depend_on_the_platform_it_is_made_on(self):
        self.assertEqual(self.package_bytes("win32"), self.package_bytes("linux"))


if __name__ == "__main__":
    unittest.main()
