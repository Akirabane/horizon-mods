"""python3 -m unittest discover -s scripts -p 'test_*.py'"""
import tomllib
import unittest

from native_compat import has_mixins_content, strip_launcher_tables

MANIFEST = """[mod]
id = "example"
version = "1.0.0"

[compatibility]
horizon_revision = "0123456789abcdef0123456789abcdef01234567"

# A trait-mod package.
[api]
path = "api"

[dependencies]
jei = { optional = true }

[dependencies.ae2]
optional = false

[[module]]
id = "runtime"
file = "crates/horizon-client/src/game.rs"
name = "example_mod"
source = "runtime.rs"

[[injection]]
id = "frame"
target = "horizon_client::game::native_hooks::Game<'_>::native_frame"
helper = "frame.rs"
"""


class StripLauncherTables(unittest.TestCase):
    def test_removes_only_the_launcher_tables(self):
        stripped = tomllib.loads(strip_launcher_tables(MANIFEST))
        original = tomllib.loads(MANIFEST)
        self.assertNotIn("api", stripped)
        self.assertNotIn("dependencies", stripped)
        for key in ("mod", "compatibility", "module", "injection"):
            self.assertEqual(stripped[key], original[key])

    def test_a_manifest_without_them_is_unchanged(self):
        plain = MANIFEST.split("# A trait-mod package.")[0] + MANIFEST.split('optional = false\n')[1]
        self.assertEqual(strip_launcher_tables(plain), plain)


class HasMixinsContent(unittest.TestCase):
    def test_a_bundle_with_injections_has_some(self):
        self.assertTrue(has_mixins_content(tomllib.loads(MANIFEST)))

    def test_an_api_only_bundle_has_none(self):
        api_only = MANIFEST.split("[dependencies]")[0]
        self.assertEqual(set(tomllib.loads(api_only)), {"mod", "compatibility", "api"})
        self.assertFalse(has_mixins_content(tomllib.loads(api_only)))


if __name__ == "__main__":
    unittest.main()
