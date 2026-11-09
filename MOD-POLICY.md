# Mod policy

This registry lists mods, modpacks and packs for [Horizon](https://github.com/horizon-rust/horizon). This page says
who makes them, who is responsible for them, and how to have one removed. By submitting to the registry you accept it.

## Who makes the mods

- **Mods made by the Horizon project** are marked **Official**: their author is "Horizon" and their source is in a
  [horizon-rust](https://github.com/horizon-rust) repository. They are the only mods the Horizon
  project makes and is responsible for.
- **Every other mod is made, owned and maintained by its own authors**, named on its page. The Horizon project and
  its maintainers are not their authors. They don't own these mods, sell them, or license them to you; each mod's
  licence is the one its authors chose.
- **A listing is not an endorsement.** Reviewers check that an entry is complete, that its files match their pinned
  hashes, that it builds or loads against the Horizon version it names, and that nothing obviously harmful is in it.
  That review is not a legal review, a security audit or a guarantee. Mods are provided as they are, without
  warranty, by their authors.
- **Native mods** (built into the game with [horizon-mixins](https://github.com/horizon-rust/horizon-mixins)) run
  with the same access to your computer as the game. Install them only from authors you trust.

## What authors promise when they submit

Submitting a project, through the form or a pull request, means you confirm that:

1. You made it, or you have the right to publish it, under the licence its entry names.
2. **If it is a port or a translation of another mod** (for example from Java Edition), the original's licence allows
   that, or its authors gave you permission. You keep the original's copyright and licence notices, and you credit
   its authors and link the original on your page. A port of a mod whose licence reserves all rights needs its
   authors' written permission.
3. It contains no code, assets or data from Minecraft, Mojang or Microsoft beyond what their
   [usage guidelines](https://www.minecraft.net/usage-guidelines) and EULA allow. Horizon players bring their own
   copy of the game.
4. It contains nothing that harms players: no malware, no stealing of accounts or data, no hidden network access.
5. The `source` link has the code the published files were built from.

You alone are responsible for what you publish and for any claim about it. The Horizon project may refuse, hide or
remove any project, at any time, without having to give a reason.

## Cheat clients

The registry does not list mods built to cheat on servers: combat and movement cheats (kill aura, reach, fly, fake
lag, velocity, auto-clickers beyond what vanilla allows) and anything built to get past a server's anticheat (for
example presets like a "GrimAC mode"). Client-side utilities that give no unfair edge on servers, such as HUDs,
seed crackers or pathfinding bots, stay welcome, as long as they carry no anticheat-bypass preset.

## Licences reviewers can read

Reviewers read a project's source before each version is listed; that is how they check it holds nothing harmful.
They may only read source under a licence that lets them, so a new project, and every new version of a listed one,
needs source under one of:

- MIT, MIT-0, BSD-2-Clause, BSD-3-Clause, 0BSD, Apache-2.0, ISC, zlib, Unlicense or CC0-1.0;
- or any licence, with a written grant from its authors that lets the Horizon reviewers read the source to review it
  (say so in the pull request, with a link to the grant).

Source under GPL, LGPL, MPL or all rights reserved, without such a grant, can't be reviewed, so it can't be listed or
updated. The projects already listed under other licences when this rule was written (classic_zoom,
distant_horizons, fly_speed, serverify) stay listed as they are; their next version needs a licence above or a
grant. A port also has to follow promise 2 above.

## Copyright and takedowns

If a project in this registry uses your work without your permission:

- [Open a takedown request](../../issues/new?template=takedown.yml) (public), or
- write privately to the maintainer through [lu4ovic.com](https://lu4ovic.com/#contact).

Say which project, which of your work it uses, and how to reach you. A claim that holds gets the project removed and
listed in [`revoked.toml`](revoked.toml): the registry stops listing it and the launcher stops offering it and turns
installed copies off. Clear claims are acted on quickly. The project's author is told, and can answer the claim in the
same issue.

Authors whose project was removed by mistake can ask for it back in that issue.

## Changes

This policy can change. The version in this repository's `main` branch is the one that applies.
