# Horizon mods

The registry of what is made for [Horizon](https://github.com/horizon-rust/horizon): mods (`.hmod`), modpacks (`.hpack`)
and resource packs, each one reviewed before it is published. Browse them at
[playhorizon.dev/mods](https://playhorizon.dev/mods) or in the launcher (Mods, then Horizon beside Modrinth).

Players install from here through the launcher, which checks every file's SHA-256 and, when the author signs, its
signature before the game sees it. Sandboxed `.hmod` mods run in Horizon's sandbox,
with no file, network or clock access, and can only do what their page lists.

Native source mods (`.hnmod`, kind `native-mod`) are built into an isolated game by
a launcher with native-mod support. They run with the player’s account permissions.
Their bundles pin the exact Horizon and Horizon Mixins revisions used for the build.
Other revisions have not been validated. Registry checks inspect the source bundle without
executing native code; reviewers must also examine the source and pinned-build
validation. Publishing these requires the native-mod changes to `cargo-horizon-mod`
and the launcher to have shipped first.

**Who is responsible.** Except the projects marked Official (made by the Horizon project), everything here is made and
owned by its own authors, not by the Horizon project or its maintainers. A listing is not an endorsement. See the
[mod policy](MOD-POLICY.md): what authors promise when they submit (ports of other mods included), the licences
whose source reviewers can read, and how to have a project taken down.

## Submit

Use the form at [playhorizon.dev/mods/submit](https://playhorizon.dev/mods/submit): sign in with GitHub, pick the file
and fill in the page. The form checks the file, uploads it here and opens a pull request for you. Then:

1. This repository's checks run on it: the file's own checks, a trial run of a mod in the sandbox, the index it would
   publish. Their report is in the pull request's checks.
2. A reviewer (see [`.github/CODEOWNERS`](.github/CODEOWNERS)) reads the entry, the source and the report, and asks
   for changes in the pull request if needed.
3. Merging publishes it: the file becomes a release, the project appears on the site and in the launcher within
   minutes.

What the registry takes now: mods, modpacks of registry mods, and resource packs. Shader packs open when Horizon's own
shader pack format is out (Iris and OptiFine packs are on Modrinth). Before you submit a mod, run what the checks run:

```sh
cargo install --git https://github.com/horizon-rust/horizon cargo-horizon-mod
cargo horizon-mod check target/horizon-mod/my_mod.hmod
```

**Updates** come the same way: the form adds a version to your entry, and a reviewer reads it before it is
published. Only an entry's `owners` can add versions, and every version of a native mod is reviewed, owner or
not: its code runs with the player's account permissions. A published file never changes: publish a new version.

**Signing** is optional and recommended: put your [minisign](https://jedisct1.github.io/minisign/) public key in your
entry, and every later version must be signed with it, so nobody else can publish an update in your name.

## Report

Something wrong with a project? [Report it](../../issues/new?template=report.yml). Your work published without your
permission? [Ask for a takedown](../../issues/new?template=takedown.yml) ([mod policy](MOD-POLICY.md)). A project that does harm is taken
down in [`revoked.toml`](revoked.toml): the launcher then refuses it and turns installed copies off, telling players
why.

## Layout

| Path | What |
| --- | --- |
| `mods/<id>.toml` | One project: its page and its versions, each pinned by SHA-256 |
| `icons/<id>.png` | Its icon, at most 256 KiB |
| `revoked.toml` | Files taken down |
| `.github/workflows/check.yml` | A pull request's checks |
| `.github/workflows/publish.yml` | On merge: releases, then `index.json` on Pages |
| `.github/workflows/cleanup.yml` | Weekly: staging files nobody needs any more |
| `.github/workflows/native.yml` | Horizon's native mods: each changed one's tests, `.hnmod` and game check, all of them together, and weekly on the newest Nightly |
| `native/<name>/` | Horizon's own native mods (Zoom, FPS Counter, Keystrokes, Minimap, Recipes, Coordinates HUD): the source of their `.hnmod` files |
| `native/tools/` | Shared by them: apply on Horizon, check, build, package, prove (`native/tools/native.py`) |
| `scripts/` | What the workflows share |

The format of the entries and of `index.json` is
[docs/api/mod-index.md](https://github.com/horizon-rust/horizon/blob/main/docs/api/mod-index.md) in Horizon's repository;
the checks are `cargo horizon-mod check` and `cargo horizon-mod registry`, from Horizon at the commit in the
repository variable `HORIZON_REV` (else `main`).

The index: `https://horizon-rust.github.io/horizon-mods/index.json`.

## Horizon's native mods

`native/<name>/` holds the source of Horizon's official mods, as
[horizon-mixins](https://github.com/horizon-rust/horizon-mixins) bundles on Horizon's native hook points
([docs/native-mods.md](https://github.com/horizon-rust/horizon/blob/main/docs/native-mods.md)). Each folder keeps the
history of the repository it came from. A mod builds on any Horizon whose hook points are at least the version its
`compatibility.json` names; `horizon_revision` is the commit it was last tested on. `smp` and `voicechat-server`
are server mods, built into the official server's engine (see their READMEs).

```sh
python3 native/tools/native.py check zoom          # apply on the tested Horizon and cargo check the game
python3 native/tools/native.py check all           # every native mod together
python3 native/tools/native.py package all         # native/target/<id>-<version>.hnmod, for a registry entry
python3 native/tools/native.py proof zoom --resources /path/to/.local/horizon-client/26.3
```

To make your own, start from [horizon-native-mod-template](https://github.com/horizon-rust/horizon-native-mod-template).
