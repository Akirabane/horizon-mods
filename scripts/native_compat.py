#!/usr/bin/env python3
"""Whether native mods build on the newest Nightly, together, as a player's launcher builds them.

    python3 scripts/native_compat.py check BASE HEAD                 # a pull request's new native versions
    python3 scripts/native_compat.py health --out health.json        # every native mod's newest version

`check` takes each native-mod version HEAD lists and BASE doesn't, and fails when:
  - one of its support modules has the name, in the same file, of another registry mod's (the launcher
    can't build both: E_MODULE_CONFLICT);
  - horizon-mixins doesn't apply it on the newest Nightly with the most downloaded native mods
    (`--popular`), as the launcher moves mods on the hook points to the Horizon it builds;
  - the game doesn't compile (`cargo check`) with it and those mods, when it does without it.
Popular mods that don't apply on that Nightly are left out, as the launcher leaves them out, and listed.

`health` applies every native mod's newest version on the newest Nightly and writes the ones horizon-mixins
refuses with only the mods they need; then `cargo check`s the rest together, rounds leaving out the mods the errors
are in (scripts/native_trace.py, as the launcher traces them), and writes those too, each with its first error. An
error of the kind two mods cause together (a name defined twice...) is checked again with that mod alone. The publish
job adds them all to the index (`broken_on`), and the launcher warns before an install.

horizon-mixins only reads the bundles; `cargo check` runs a mod crate's build script, if it has one, so
`check` runs in a job whose token can only read (native-compat.yml). Horizon's source comes from GitHub's archive.
"""
import argparse
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "native" / "tools"))
import native  # noqa: E402  (native/tools/native.py: source, latest_nightly, get, log)
import native_trace  # noqa: E402

INDEX = "https://horizon-rust.github.io/horizon-mods/index.json"
REGISTRY = "horizon-rust/horizon-mods"
MIXINS_GIT = "https://github.com/horizon-rust/horizon-mixins"
WORK = native.TARGET / "compat"


def log(*args):
    native.log(*args)


class Bundle:
    """An unpacked `.hnmod`: its mixin bundle in WORK/mods/<set>/<id>/."""

    def __init__(self, data, origin):
        archive = zipfile.ZipFile(io.BytesIO(data))
        meta = json.loads(archive.read("horizon-native.json"))
        self.id, self.version, self.origin = meta["id"], meta["version"], origin
        self.mixins = meta.get("mixins")
        prefix = f"mixins/{self.id}/"
        self.files = {n[len(prefix):]: archive.read(n) for n in archive.namelist()
                      if n.startswith(prefix) and not n.endswith("/")}
        manifest = tomllib.loads(self.files["horizon-mixin.toml"].decode())
        self.modules = {(m["file"], m["name"]) for m in manifest.get("module", [])}
        self.needs = [d for d, v in manifest.get("dependencies", {}).items()
                      if not (isinstance(v, dict) and v.get("optional"))]
        # The launcher rewrites these before horizon-mixins and stages API packages for them (native_mods/inject.rs,
        # api.rs), which this script doesn't: they are applied here, not compiled.
        self.api = "api" in manifest or any(isinstance(i, dict) and "trait_method" in i
                                            for i in manifest.get("injection", []))
        self.mixins_content = has_mixins_content(manifest)

    def unpack(self, into, revision):
        out = into / self.id
        for name, data in self.files.items():
            path = out / name
            if ".." in Path(name).parts:
                sys.exit(f"{self.id}: {name} leaves its folder")
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        # As the launcher does for a mod built on another Horizon than it pins.
        toml = out / "horizon-mixin.toml"
        text = re.sub(r'horizon_revision\s*=\s*"[0-9a-f]{40}"', f'horizon_revision = "{revision}"', toml.read_text())
        toml.write_text(strip_launcher_tables(text))


# The tables only the launcher reads; it removes them before horizon-mixins sees the manifest (native_mods/api.rs).
LAUNCHER_TABLES = ("api", "dependencies")


def has_mixins_content(manifest):
    """Whether horizon-mixins has anything to apply in this manifest. The launcher leaves out a bundle with only
    [mod], [compatibility] and its own tables, e.g. one that is only an [api] package (native_mods.rs,
    `mixins_content`); horizon-mixins refuses such a manifest (E_MANIFEST)."""
    return any(key not in ("mod", "compatibility", *LAUNCHER_TABLES) for key in manifest)


def strip_launcher_tables(text):
    """`text` without its [api] and [dependencies] tables (and their sub-tables), the rest unchanged."""
    kept, skipping = [], False
    for line in text.splitlines(keepends=True):
        header = re.match(r'\s*\[\[?\s*([^\]]+?)\s*\]\]?\s*(#.*)?$', line)
        if header:
            skipping = header.group(1).split(".")[0].strip().strip('"\'') in LAUNCHER_TABLES
        if not skipping:
            kept.append(line)
    stripped = "".join(kept)
    want = {k: v for k, v in tomllib.loads(text).items() if k not in LAUNCHER_TABLES}
    if tomllib.loads(stripped) != want:
        sys.exit("horizon-mixin.toml: its [api] or [dependencies] table couldn't be removed line by line")
    return stripped


def download(url, sha256):
    cache = WORK / "files" / sha256
    if not cache.is_file():
        data = native.get(url)
        if hashlib.sha256(data).hexdigest() != sha256:
            sys.exit(f"{url}: its SHA-256 isn't the index's")
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_bytes(data)
    return cache.read_bytes()


def published():
    """The newest version of every native mod in the published index, most downloaded first."""
    index = json.loads(native.get(INDEX))
    revoked = {r.get("sha256") for r in index.get("revoked", [])}
    found = []
    for project in sorted(index["projects"], key=lambda p: -p.get("downloads", 0)):
        if project.get("kind") != "native-mod":
            continue
        version = next((v for v in project["versions"] if v["file"]["sha256"] not in revoked), None)
        if version:
            found.append(Bundle(download(version["file"]["url"], version["file"]["sha256"]), "registry"))
    return found


def mixins_tool(bundles):
    """horizon-mixins as the launcher picks it: the newest of its own (Horizon main's MIXINS_REV) and the
    mods' pins, by commit date."""
    text = native.get(f"https://raw.githubusercontent.com/{native.REPO}/main/launcher/src/native_mods.rs").decode()
    revs = {re.search(r'pub const MIXINS_REV: &str = "([0-9a-f]{40})"', text).group(1)}
    revs |= {b.mixins for b in bundles if b.mixins}
    def date(rev):
        commit = json.loads(native.get(f"https://api.github.com/repos/horizon-rust/horizon-mixins/commits/{rev}"))
        return commit["commit"]["committer"]["date"]
    rev = max(revs, key=date)
    root = native.TARGET / "horizon-mixins" / rev
    binary = root / "bin" / "horizon-mixins"
    if not binary.is_file():
        log(f"Building horizon-mixins {rev[:7]}")
        subprocess.run(["cargo", "install", "--locked", "--quiet", "--git", MIXINS_GIT, "--rev", rev,
                        "--root", str(root), "horizon-mixins-cli"], check=True)
    return binary


def culprit(diagnostic, bundles):
    """The bundle a horizon-mixins diagnostic names: by mod id, its manifest's path, its module's name or
    destination; between two mods' modules, the later one (the launcher's rule)."""
    by_id = {b.id: b for b in bundles}
    if diagnostic.get("mod_id") in by_id:
        return by_id[diagnostic["mod_id"]]
    file = diagnostic.get("file", "")
    for b in bundles:
        if f"/{b.id}/horizon-mixin.toml" in file:
            return b
    named = re.search(r"support module '([^']+)'", diagnostic.get("message", ""))
    declaring = [b for b in bundles if any((named and n == named.group(1)) or f == file for f, n in b.modules)]
    return declaring[-1] if declaring else None


def apply(tool, tree, bundles, revision, name):
    """horizon-mixins on `bundles` together, leaving out each one it refuses (and the ones needing it) until
    the rest applies: (stage folder or None, {id: diagnostic}). Bundles with nothing for horizon-mixins are left
    out, as the launcher leaves them out."""
    left, refused = [b for b in bundles if b.mixins_content], {}
    while left:
        mods = WORK / "mods" / name
        shutil.rmtree(mods, ignore_errors=True)
        mods.mkdir(parents=True)
        for b in left:
            b.unpack(mods, revision)
        out = WORK / "stage" / name
        shutil.rmtree(out, ignore_errors=True)
        run = subprocess.run([str(tool), "apply", "--workspace", str(tree), "--mods", str(mods), "--out", str(out)],
                             capture_output=True, text=True)
        if run.returncode == 0:
            return out, refused
        text = run.stderr + run.stdout
        try:
            diagnostic = json.loads(text[text.index("{"):text.rindex("}") + 1])
        except ValueError:
            sys.exit(f"horizon-mixins failed without a diagnostic:\n{text[-2000:]}")
        bad = culprit(diagnostic, left)
        if bad is None:
            sys.exit(f"horizon-mixins refused the set, naming no mod: {json.dumps(diagnostic)}")
        log(f"  {bad.id} {bad.version}: {diagnostic.get('code')}: {diagnostic.get('message')}")
        refused[bad.id] = diagnostic
        gone = {bad.id}
        while True:
            more = [b for b in left if b.id not in gone and any(n in gone for n in b.needs)]
            if not more:
                break
            for b in more:
                refused[b.id] = {"code": "E_DEPENDENCY", "message": f"needs {', '.join(b.needs)}"}
                gone.add(b.id)
        left = [b for b in left if b.id not in gone]
    return None, refused


# Errors two mods' code can cause together, while each alone compiles.
PAIR_CODES = {"E0119", "E0201", "E0252", "E0254", "E0255", "E0260", "E0428", "E0592"}
COMPILE_ROUNDS = 6


def check_errors(stage):
    """`cargo check` of the game in `stage` as the launcher compiles it (with Horizon's `mods` feature when it has
    one): None when it compiles, else its errors (native_trace.errors). The workspace is synced into one folder
    first, so cargo's incremental state survives between rounds."""
    work = WORK / "check"
    work.mkdir(parents=True, exist_ok=True)
    subprocess.run(["rsync", "-a", "--checksum", "--delete", f"{stage}/", f"{work}/"], check=True)
    command = ["cargo", "check", "--locked", "--message-format=json", "-p", "horizon-client", "--bin", "horizon-client"]
    client = tomllib.loads((work / "crates/horizon-client/Cargo.toml").read_text())
    if "mods" in client.get("features", {}):
        command += ["--features", "horizon-client/mods"]
    env = dict(os.environ, CARGO_TARGET_DIR=str(WORK / "cargo"))
    run = subprocess.run(command, cwd=work, env=env, capture_output=True, text=True)
    if run.returncode == 0:
        return None
    found = native_trace.errors(run.stdout)
    for e in found[:10]:
        log(e["rendered"].rstrip())
    return found or [{"code": "E_CARGO", "message": run.stderr[-2000:], "rendered": run.stderr[-2000:], "spans": []}]


def compiles(stage):
    return stage is not None and check_errors(stage) is None


def traced(errors, tree, mods):
    """{id: its first error} for the errors in `mods`' own code (native_trace)."""
    tracer = native_trace.Tracer(WORK / "check", tree, {(b.id, b.version) for b in mods})
    found = {}
    for e in errors:
        for span in e["spans"]:
            owner = tracer.owner(span)
            if owner and owner not in found:
                where = f" ({span['file_name']}:{span.get('line_start')})"
                found[owner] = {"code": e["code"], "message": e["message"] + where}
    return found


def broken_compiling(tool, tree, mods, revision, needed):
    """The mods of `mods` (all applying) whose own code doesn't compile on `revision`: {id: error}."""
    by_id = {b.id: b for b in mods}
    left, broken = list(mods), {}
    for _ in range(COMPILE_ROUNDS):
        stage, _ = apply(tool, tree, left, revision, "health-compile")
        if stage is None:
            break
        errors = check_errors(stage)
        if errors is None:
            log(f"  {len(left)} native mods compile together")
            return broken
        found = traced(errors, tree, left)
        if not found:
            log("  the errors are in no mod's code: stopping the compile check here")
            break
        for i, error in sorted(found.items()):
            if error["code"] in PAIR_CODES:
                alone = [by_id[n] for n in sorted(needed(by_id[i], set())) if n in by_id] + [by_id[i]]
                stage, _ = apply(tool, tree, alone, revision, f"health-alone-{i}")
                if stage is None or i not in traced(check_errors(stage) or [], tree, alone):
                    log(f"  {i}: {error['code']} only next to other mods: not broken")
                    continue
            log(f"  {i} {by_id[i].version}: {error['code']}: {error['message']}")
            broken[i] = error
        gone = set(found)
        while True:
            more = {b.id for b in left if b.id not in gone and any(n in gone for n in b.needs)}
            if not more:
                break
            for i in more:
                if any(n in broken for n in by_id[i].needs):
                    broken[i] = {"code": "E_DEPENDENCY", "message": f"needs {', '.join(by_id[i].needs)}"}
            gone |= more
        left = [b for b in left if b.id not in gone]
    return broken


def new_entries(base, head):
    """(id, version, file, sha256) of each native-mod version HEAD lists and BASE doesn't."""
    def versions(root):
        out = {}
        for path in sorted((Path(root) / "mods").glob("*.toml")):
            entry = tomllib.loads(path.read_text())
            if entry.get("kind") == "native-mod":
                for v in entry.get("versions", []):
                    out[(entry["id"], v["version"])] = (v["file"], v["sha256"])
        return out
    before = versions(base)
    return [(i, v, f, s) for (i, v), (f, s) in versions(head).items() if (i, v) not in before]


def new_versions(base, head):
    """The native-mod versions HEAD lists that BASE doesn't: their file from its release once published, else
    from the `staging` prerelease the submission form uploads it to (scripts/fetch.sh)."""
    found = []
    for id, version, file, sha in new_entries(base, head):
        releases = f"https://github.com/{REGISTRY}/releases/download"
        try:
            data = download(f"{releases}/{id}-{version}/{file}", sha)
        except Exception:
            data = download(f"{releases}/staging/{sha}.hnmod", sha)
        found.append(Bundle(data, "submitted"))
    return found


def check(args):
    submitted = new_versions(args.base, args.head)
    if not submitted:
        print("No new native mod version.")
        return 0
    registry = published()
    revision = native.latest_nightly() if args.horizon == "latest" else args.horizon
    tree = native.source(revision)
    failed = []
    for mod in submitted:
        others = [b for b in registry if b.id != mod.id]
        print(f"## {mod.id} {mod.version} on Horizon {revision[:7]} (the newest Nightly)")
        clashes = sorted({(b.id, f, n) for b in others for f, n in mod.modules & b.modules})
        for other, file, name in clashes:
            print(f"- support module `{name}` in `{file}` is also {other}'s: the launcher can't build both."
                  f" Name it after the mod (`{mod.id}_…`).")
        popular = others[:args.popular]
        tool = mixins_tool([mod, *popular])
        stage, refused = apply(tool, tree, [mod, *popular], revision, f"{mod.id}-together")
        for other, diagnostic in refused.items():
            if other != mod.id:
                print(f"- {other} (popular) doesn't apply on this Nightly either: {diagnostic.get('code')}."
                      " The launcher leaves it out.")
        if mod.id in refused:
            d = refused[mod.id]
            print(f"- **doesn't apply**: `{d.get('code')}`: {d.get('message')}"
                  + (f" (injection `{d['injection_id']}`)" if d.get("injection_id") else ""))
            failed.append(mod.id)
            continue
        if clashes:
            failed.append(mod.id)
        if args.no_compile:
            continue
        if mod.api:
            # Its API package is staged by the launcher only (Bundle.api), as `health` leaves these out too.
            print("- applies; it has a mod API package, which only the launcher builds, so it isn't compiled here.")
            continue
        if compiles(stage):
            print(f"- applies and compiles with {len(popular) - len(refused)} popular mods.")
            continue
        alone, _ = apply(tool, tree, [mod], revision, f"{mod.id}-alone")
        without, _ = apply(tool, tree, [b for b in popular if b.id not in refused], revision, f"{mod.id}-without")
        if not compiles(alone):
            print("- **doesn't compile** on this Nightly, even alone (the errors are in the log).")
            failed.append(mod.id)
        elif without is None or compiles(without):
            print("- **doesn't compile together with the popular mods**, while each side compiles alone.")
            failed.append(mod.id)
        else:
            print("- compiles alone; the popular mods already don't compile together on this Nightly.")
    return 1 if failed else 0


def health(args):
    registry = published()
    revision = native.latest_nightly() if args.horizon == "latest" else args.horizon
    tree = native.source(revision)
    tool = mixins_tool([])
    _, together = apply(tool, tree, registry, revision, "health")
    # A mod refused only next to another (both overwrite one function, both name one module) builds for a
    # player without that other one: broken on this Horizon is what horizon-mixins refuses with only the mods
    # it needs.
    by_id = {b.id: b for b in registry}
    def needed(bundle, seen):
        for n in bundle.needs:
            if n in by_id and n not in seen:
                seen.add(n)
                needed(by_id[n], seen)
        return seen
    refused = {}
    for i in sorted(together):
        mods = [by_id[n] for n in sorted(needed(by_id[i], set()))] + [by_id[i]]
        _, alone = apply(tool, tree, mods, revision, f"health-{i}")
        if i in alone:
            refused[i] = alone[i]
        else:
            log(f"  {i} applies without the others: not broken")
    # The rest, compiled; without the mods the launcher prepares differently (Bundle.api) and those needing them.
    skipped = {b.id for b in registry if b.api}
    while more := {b.id for b in registry if b.id not in skipped and any(n in skipped for n in b.needs)}:
        skipped |= more
    if skipped:
        log(f"  not compiled here (an API the launcher prepares): {', '.join(sorted(skipped))}")
    compiling = [b for b in registry if b.id not in refused and b.id not in skipped]
    failed = broken_compiling(tool, tree, compiling, revision, needed) if args.compile else {}
    broken = {i: dict(d, stage="apply") for i, d in refused.items()}
    broken |= {i: dict(d, stage="compile") for i, d in failed.items()}
    report = {
        "horizon": revision,
        "mixins": tool.parent.parent.name,
        "broken": {i: {"version": by_id[i].version, "code": d.get("code"), "message": d.get("message"),
                       "stage": d["stage"]}
                   for i, d in sorted(broken.items())},
    }
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n")
    print(f"{len(broken)} of {len(registry)} native mods don't build on Horizon {revision[:7]}: "
          f"{len(refused)} don't apply ({', '.join(sorted(refused))}), {len(failed)} don't compile "
          f"({', '.join(sorted(failed))}).")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="action", required=True)
    c = sub.add_parser("check")
    c.add_argument("base")
    c.add_argument("head")
    c.add_argument("--popular", type=int, default=20)
    c.add_argument("--no-compile", action="store_true")
    n = sub.add_parser("new", help="list the new native-mod versions, one per line")
    n.add_argument("base")
    n.add_argument("head")
    h = sub.add_parser("health")
    h.add_argument("--out", required=True)
    h.add_argument("--no-compile", dest="compile", action="store_false")
    for p in (c, h):
        p.add_argument("--horizon", default="latest", help="a Horizon commit, or `latest` (the newest Nightly)")
    args = parser.parse_args()
    if args.action == "new":
        for id, version, _, _ in new_entries(args.base, args.head):
            print(id, version)
        return
    sys.exit(check(args) if args.action == "check" else health(args))


if __name__ == "__main__":
    main()
