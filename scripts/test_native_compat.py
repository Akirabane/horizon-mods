"""python3 -m unittest discover -s scripts -p 'test_*.py'"""
import tomllib
import unittest

from native_compat import has_mixins_content, newest_unrevoked, strip_launcher_tables

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


def project(id, *hashes, kind="native-mod", downloads=0):
    return {"id": id, "kind": kind, "downloads": downloads,
            "versions": [{"version": f"1.{i}", "file": {"sha256": h}} for i, h in enumerate(hashes)]}


class NewestUnrevoked(unittest.TestCase):
    def listed(self, projects, revoked):
        return [(p["id"], v["file"]["sha256"]) for p, v in newest_unrevoked(projects, revoked)]

    def test_an_id_without_a_hash_leaves_every_version_out(self):
        projects = [project("nea", "a1", "a2"), project("other", "b1")]
        revoked = [{"id": "nea", "reason": "taken down"}]
        self.assertEqual(self.listed(projects, revoked), [("other", "b1")])

    def test_a_hash_leaves_out_only_that_file(self):
        projects = [project("baritone", "new", "old")]
        revoked = [{"id": "baritone", "sha256": "new"}]
        self.assertEqual(self.listed(projects, revoked), [("baritone", "old")])

    def test_a_project_with_only_revoked_files_is_left_out(self):
        revoked = [{"id": "x", "sha256": "x1"}]
        self.assertEqual(self.listed([project("x", "x1")], revoked), [])

    def test_only_native_mods_most_downloaded_first(self):
        projects = [project("few", "f", downloads=1), project("content", "c", kind="content-mod", downloads=99),
                    project("many", "m", downloads=50)]
        self.assertEqual(self.listed(projects, []), [("many", "m"), ("few", "f")])


if __name__ == "__main__":
    unittest.main()
