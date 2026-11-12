// Operators' lookups of any player, on the server or not: `/smpadmin claims
// <player>` (their chunks, joined into areas, and who they trust),
// `/smpadmin homes <player>` and `/smpadmin lastpos <player>` (where they
// last left the game). Where each player left is kept in
// `last_positions.json`, written when they leave. Each place is a line an
// operator clicks to go there. No game types. Added to the game as
// `game::smp_admin`; also a module of this crate, for its tests.

use super::smp_claims::Claims;
use super::smp_economy::Player;
use super::smp_places::{Place, Places};
use super::smp_store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const COMMANDS: [&str; 1] = ["smpadmin"];

/// The operator level that looks players up.
pub const LEVEL: u8 = 3;

/// The usages, for the command tree (all at [`LEVEL`]).
pub const USAGES: [&str; 4] = [
    "smpadmin",
    "smpadmin claims <player:word>",
    "smpadmin homes <player:word>",
    "smpadmin lastpos <player:word>",
];

/// Where a player last left the game.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastPosition {
    /// Their name then.
    pub name: String,
    pub dimension: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// When (ms since 1970).
    pub at: i64,
}

/// `last_positions.json`: by player UUID.
pub struct LastPositions {
    path: PathBuf,
    data: BTreeMap<String, LastPosition>,
    dirty: bool,
}

impl LastPositions {
    /// `last_positions.json` in `dir`; an error when it exists but can't be
    /// read (never replaced by an empty one).
    pub fn open(dir: &Path) -> Result<Self, String> {
        let path = dir.join("last_positions.json");
        let data = smp_store::read_json(&path)?;
        Ok(Self {
            path,
            data,
            dirty: false,
        })
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.dirty {
            smp_store::save_json(&self.path, &self.data)?;
            self.dirty = false;
        }
        Ok(())
    }

    /// `uuid` (`name`) left the game at `at`.
    pub fn left(&mut self, uuid: u128, name: &str, at: &Place, now: i64) {
        self.data.insert(
            smp_store::uuid_string(uuid),
            LastPosition {
                name: name.to_owned(),
                dimension: at.dimension.clone(),
                x: at.x,
                y: at.y,
                z: at.z,
                at: now,
            },
        );
        self.dirty = true;
    }

    pub fn get(&self, uuid: u128) -> Option<&LastPosition> {
        self.data.get(&smp_store::uuid_string(uuid))
    }

    /// A player by the name they had when they last left (any case); the
    /// one who left last when two had it.
    pub fn find(&self, name: &str) -> Option<(u128, String)> {
        self.data
            .iter()
            .filter(|(_, p)| p.name.eq_ignore_ascii_case(name))
            .max_by_key(|(_, p)| p.at)
            .and_then(|(k, p)| Some((smp_store::parse_uuid(k)?, p.name.clone())))
    }
}

/// Where a click takes the operator.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    /// Exactly there (a home, where a player stood).
    Exact(Place),
    /// A column whose ground isn't known (a claim): the command is put in
    /// the chat box with the operator's own height (`~`), as vanilla's
    /// `/locate` link does, to change before running it.
    Column { dimension: String, x: i32, z: i32 },
}

impl Go {
    /// The command the click runs (`Exact`) or suggests (`Column`).
    pub fn command(&self) -> String {
        match self {
            Go::Exact(p) => format!(
                "/execute in {} run tp @s {:.3} {:.3} {:.3}",
                p.dimension, p.x, p.y, p.z
            ),
            Go::Column { dimension, x, z } => {
                format!("/execute in {dimension} run tp @s {x} ~ {z}")
            }
        }
    }
}

/// A line of an answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub error: bool,
    /// Where clicking the line goes.
    pub go: Option<Go>,
}

impl Line {
    fn say(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
            go: None,
        }
    }
    fn error(text: impl Into<String>) -> Self {
        Self {
            error: true,
            ..Self::say(text)
        }
    }
    fn go(text: impl Into<String>, go: Go) -> Self {
        Self {
            go: Some(go),
            ..Self::say(text)
        }
    }
}

/// What the lookups read: each part, or why it is off.
pub struct Lookup<'a> {
    pub claims: Result<&'a Claims, String>,
    pub places: Result<&'a Places, String>,
    pub last: Result<&'a LastPositions, String>,
    /// A player by name (any case): on now, then everyone the server knows
    /// (accounts, `usercache.json`); `last_positions.json` after them.
    pub find: &'a dyn Fn(&str) -> Option<(u128, String)>,
    /// A UUID's name, for the trusted players.
    pub name_of: &'a dyn Fn(u128) -> String,
    /// Where a player on now stands.
    pub online: &'a dyn Fn(u128) -> Option<Place>,
}

/// `overworld 10 64 -5`.
fn where_text(dimension: &str, x: f64, y: f64, z: f64) -> String {
    format!(
        "{} {} {} {}",
        dimension.trim_start_matches("minecraft:"),
        x.floor() as i64,
        y.floor() as i64,
        z.floor() as i64
    )
}

/// `1 chunk`, `4 chunks`.
fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// `3 h 05 min`, `12 min`, `2 d 4 h`, for how long ago.
fn ago(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m:02} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{s} s")
    }
}

/// Chunks touching side by side, as areas: each a sorted list of chunks.
fn areas(chunks: &[(i32, i32)]) -> Vec<Vec<(i32, i32)>> {
    let mut left: BTreeSet<(i32, i32)> = chunks.iter().copied().collect();
    let mut out = Vec::new();
    while let Some(&start) = left.iter().next() {
        left.remove(&start);
        let (mut area, mut todo) = (vec![start], vec![start]);
        while let Some((x, z)) = todo.pop() {
            for n in [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)] {
                if left.remove(&n) {
                    area.push(n);
                    todo.push(n);
                }
            }
        }
        area.sort_unstable();
        out.push(area);
    }
    out
}

/// `/smpadmin ...`; None for another command.
pub fn command(me: &Player, look: &Lookup, now: i64, line: &str) -> Option<Vec<Line>> {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.first() != Some(&"smpadmin") {
        return None;
    }
    if me.level < LEVEL {
        return Some(vec![Line::error("Only operators may use /smpadmin.")]);
    }
    let usage = || {
        vec![Line::error(
            "Usage: /smpadmin claims <player>, /smpadmin homes <player>, /smpadmin lastpos <player>",
        )]
    };
    let [what, name] = words[1..] else {
        return Some(usage());
    };
    if !matches!(what, "claims" | "homes" | "lastpos") {
        return Some(usage());
    }
    let found = (look.find)(name).or_else(|| look.last.as_ref().ok()?.find(name));
    let Some((uuid, name)) = found else {
        return Some(vec![Line::error(format!(
            "No player named {name} has played here."
        ))]);
    };
    Some(match what {
        "claims" => claims(look, uuid, &name),
        "homes" => homes(look, uuid, &name),
        _ => lastpos(look, uuid, &name, now),
    })
}

fn claims(look: &Lookup, uuid: u128, name: &str) -> Vec<Line> {
    let c = match &look.claims {
        Ok(c) => c,
        Err(e) => return vec![Line::error(format!("Claims are off: {e}"))],
    };
    let chunks = c.chunks_of(uuid);
    let played = look.places.as_ref().map_or(0, |p| p.played_seconds(uuid));
    let mut by_dimension: BTreeMap<&str, Vec<(i32, i32)>> = BTreeMap::new();
    for (d, x, z) in &chunks {
        by_dimension.entry(d).or_default().push((*x, *z));
    }
    let all: Vec<(&str, Vec<(i32, i32)>)> = by_dimension
        .iter()
        .flat_map(|(d, cs)| areas(cs).into_iter().map(move |a| (*d, a)))
        .collect();
    let mut out = vec![Line::say(format!(
        "{name}'s claims: {} in {} (may hold {}).",
        count(chunks.len(), "chunk"),
        count(all.len(), "area"),
        c.limit(played)
    ))];
    for (dimension, area) in &all {
        let min_x = area.iter().map(|c| c.0).min().unwrap_or(0) * 16;
        let max_x = area.iter().map(|c| c.0).max().unwrap_or(0) * 16 + 15;
        let min_z = area.iter().map(|c| c.1).min().unwrap_or(0) * 16;
        let max_z = area.iter().map(|c| c.1).max().unwrap_or(0) * 16 + 15;
        out.push(Line::go(
            format!(
                "{}: x {min_x} to {max_x}, z {min_z} to {max_z} ({}, {}x{} blocks)",
                dimension.trim_start_matches("minecraft:"),
                count(area.len(), "chunk"),
                max_x - min_x + 1,
                max_z - min_z + 1
            ),
            Go::Column {
                dimension: (*dimension).to_owned(),
                x: (min_x + max_x + 1).div_euclid(2),
                z: (min_z + max_z + 1).div_euclid(2),
            },
        ));
    }
    let trusted: Vec<String> = c
        .trusted_by(uuid)
        .into_iter()
        .map(|u| (look.name_of)(u))
        .collect();
    out.push(Line::say(format!(
        "Trusted: {}",
        if trusted.is_empty() {
            "nobody".into()
        } else {
            trusted.join(", ")
        }
    )));
    out
}

fn homes(look: &Lookup, uuid: u128, name: &str) -> Vec<Line> {
    let p = match &look.places {
        Ok(p) => p,
        Err(e) => return vec![Line::error(format!("Homes are off: {e}"))],
    };
    let homes = p.homes_of(uuid);
    let mut out = vec![Line::say(format!(
        "{name}'s homes ({} of {}){}",
        homes.len(),
        p.home_limit(uuid),
        if homes.is_empty() { ": none." } else { ":" }
    ))];
    for (home, at) in homes {
        out.push(Line::go(
            format!("{home}: {}", where_text(&at.dimension, at.x, at.y, at.z)),
            Go::Exact(at),
        ));
    }
    out
}

fn lastpos(look: &Lookup, uuid: u128, name: &str, now: i64) -> Vec<Line> {
    let mut out = Vec::new();
    if let Some(at) = (look.online)(uuid) {
        out.push(Line::go(
            format!(
                "{name} is on now: {}",
                where_text(&at.dimension, at.x, at.y, at.z)
            ),
            Go::Exact(at),
        ));
    }
    match &look.last {
        Err(e) => out.push(Line::error(format!("Last positions are off: {e}"))),
        Ok(l) => match l.get(uuid) {
            Some(p) => out.push(Line::go(
                format!(
                    "{name} last left {} UTC ({} ago) at {}",
                    smp_store::utc_minute(p.at),
                    ago(now - p.at),
                    where_text(&p.dimension, p.x, p.y, p.z)
                ),
                Go::Exact(Place {
                    dimension: p.dimension.clone(),
                    x: p.x,
                    y: p.y,
                    z: p.z,
                }),
            )),
            None => out.push(Line::say(format!(
                "No logoff of {name} kept yet (kept since SMP 0.36.0)."
            ))),
        },
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{smp_claims, smp_places};
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const OP: u128 = 1;
    const STEVE: u128 = 2;
    const ALEX: u128 = 3;
    /// Known only from where he last left.
    const GONE: u128 = 4;
    const NOW: i64 = 1_793_880_000_000;

    fn dir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("smp-admin-{}-{n}", std::process::id()))
    }
    fn player(uuid: u128, level: u8) -> Player {
        let name = match uuid {
            OP => "Op",
            STEVE => "Steve",
            _ => "Alex",
        };
        Player {
            uuid,
            name: name.into(),
            level,
        }
    }
    fn at(dimension: &str, x: f64, y: f64, z: f64) -> Place {
        Place {
            dimension: dimension.into(),
            x,
            y,
            z,
        }
    }
    /// The server knows Steve and Alex (accounts, usercache); nobody is on.
    fn find(n: &str) -> Option<(u128, String)> {
        match n.to_ascii_lowercase().as_str() {
            "steve" => Some((STEVE, "Steve".into())),
            "alex" => Some((ALEX, "Alex".into())),
            _ => None,
        }
    }
    fn name_of(u: u128) -> String {
        match u {
            STEVE => "Steve",
            ALEX => "Alex",
            _ => "?",
        }
        .into()
    }
    fn nobody_on(_: u128) -> Option<Place> {
        None
    }

    /// Three homes from the start.
    fn homes3() -> smp_places::Config {
        smp_places::Config {
            homes: vec![[0, 3]],
            ..smp_places::Config::default()
        }
    }

    /// Steve's world: two touching chunks and one apart in the Overworld,
    /// one in the Nether, Alex trusted, two homes; then he logs off.
    fn steve_world(d: &Path) -> (Claims, Places, LastPositions) {
        let mut c = Claims::open(d, smp_claims::Config::default()).unwrap();
        let mut p = Places::open(d, homes3()).unwrap();
        let steve = player(STEVE, 0);
        let fnd = |n: &str| find(n);
        let nm = |u: u128| name_of(u);
        for (dim, x, z) in [
            ("minecraft:overworld", 5.0, 5.0),
            ("minecraft:overworld", 20.0, 5.0),
            ("minecraft:overworld", 100.0, -40.0),
            ("minecraft:the_nether", -3.0, -3.0),
        ] {
            let out = c
                .command(&steve, &at(dim, x, 64.0, z), 0, &fnd, &nm, "claim")
                .unwrap();
            assert!(!out.is_empty());
        }
        c.command(
            &steve,
            &at("minecraft:overworld", 5.0, 64.0, 5.0),
            0,
            &fnd,
            &nm,
            "trust alex",
        )
        .unwrap();
        p.played(&[STEVE], NOW);
        for (name, x) in [("base", 10.5), ("farm", -200.25)] {
            let me = smp_places::Body {
                uuid: STEVE,
                name: "Steve".into(),
                at: at("minecraft:overworld", x, 70.0, 3.0),
                health: 20.0,
            };
            p.command(&steve, &[me], NOW, &format!("sethome {name}"))
                .unwrap();
        }
        let mut l = LastPositions::open(d).unwrap();
        l.left(
            STEVE,
            "Steve",
            &at("minecraft:the_nether", 1.5, 40.0, -7.9),
            NOW - 3_600_000,
        );
        l.left(
            GONE,
            "Notch",
            &at("minecraft:overworld", 0.0, 64.0, 0.0),
            NOW - 60_000,
        );
        (c, p, l)
    }

    fn run(look: &Lookup, level: u8, line: &str) -> Vec<Line> {
        command(&player(OP, level), look, NOW, line).unwrap()
    }

    #[test]
    fn an_operator_sees_an_offline_players_claims_homes_and_logoff() {
        let d = dir();
        let (c, p, l) = steve_world(&d);
        let look = Lookup {
            claims: Ok(&c),
            places: Ok(&p),
            last: Ok(&l),
            find: &find,
            name_of: &name_of,
            online: &nobody_on,
        };
        let claims = run(&look, 3, "smpadmin claims steve");
        let texts: Vec<&str> = claims.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Steve's claims: 4 chunks in 3 areas (may hold 4).",
                "overworld: x 0 to 31, z 0 to 15 (2 chunks, 32x16 blocks)",
                "overworld: x 96 to 111, z -48 to -33 (1 chunk, 16x16 blocks)",
                "the_nether: x -16 to -1, z -16 to -1 (1 chunk, 16x16 blocks)",
                "Trusted: Alex",
            ]
        );
        assert_eq!(
            claims[1].go.as_ref().unwrap().command(),
            "/execute in minecraft:overworld run tp @s 16 ~ 8"
        );
        assert_eq!(
            claims[3].go.as_ref().unwrap().command(),
            "/execute in minecraft:the_nether run tp @s -8 ~ -8"
        );

        let homes = run(&look, 3, "smpadmin homes Steve");
        let texts: Vec<&str> = homes.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Steve's homes (2 of 3):",
                "base: overworld 10 70 3",
                "farm: overworld -201 70 3",
            ]
        );
        assert_eq!(
            homes[2].go.as_ref().unwrap().command(),
            "/execute in minecraft:overworld run tp @s -200.250 70.000 3.000"
        );

        let last = run(&look, 3, "smpadmin lastpos STEVE");
        assert_eq!(last.len(), 1, "not on: only the logoff");
        assert_eq!(
            last[0].text,
            format!(
                "Steve last left {} UTC (1 h 00 min ago) at the_nether 1 40 -8",
                smp_store::utc_minute(NOW - 3_600_000)
            )
        );
        assert_eq!(
            last[0].go.as_ref().unwrap().command(),
            "/execute in minecraft:the_nether run tp @s 1.500 40.000 -7.900"
        );
        // Alex has played but has nothing.
        let alex = run(&look, 3, "smpadmin claims alex");
        assert_eq!(
            alex[0].text,
            "Alex's claims: 0 chunks in 0 areas (may hold 4)."
        );
        assert_eq!(
            run(&look, 3, "smpadmin lastpos alex")[0].text,
            "No logoff of Alex kept yet (kept since SMP 0.36.0)."
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_player_known_only_from_their_logoff_is_found() {
        let d = dir();
        let (c, p, l) = steve_world(&d);
        let look = Lookup {
            claims: Ok(&c),
            places: Ok(&p),
            last: Ok(&l),
            find: &find,
            name_of: &name_of,
            online: &nobody_on,
        };
        let out = run(&look, 4, "smpadmin lastpos notch");
        assert!(!out[0].error, "{out:?}");
        assert!(out[0].text.starts_with("Notch last left "), "{out:?}");
        assert!(
            out[0].text.ends_with("(1 min ago) at overworld 0 64 0"),
            "{out:?}"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_player_on_now_shows_where_they_stand_too() {
        let d = dir();
        let (c, p, l) = steve_world(&d);
        let here = |u: u128| (u == STEVE).then(|| at("minecraft:overworld", 7.0, 65.0, 8.0));
        let look = Lookup {
            claims: Ok(&c),
            places: Ok(&p),
            last: Ok(&l),
            find: &find,
            name_of: &name_of,
            online: &here,
        };
        let out = run(&look, 3, "smpadmin lastpos steve");
        assert_eq!(out[0].text, "Steve is on now: overworld 7 65 8");
        assert!(out[1].text.starts_with("Steve last left "));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn logoffs_are_kept_over_a_restart() {
        let d = dir();
        let mut l = LastPositions::open(&d).unwrap();
        l.left(
            STEVE,
            "Steve",
            &at("minecraft:the_end", 100.0, 50.0, 0.5),
            NOW,
        );
        // A later logoff replaces the first.
        l.left(
            STEVE,
            "Steve",
            &at("minecraft:overworld", -1.0, 64.0, 2.0),
            NOW + 1,
        );
        l.save().unwrap();
        assert_eq!(smp_store::flush_dir(&d), Vec::<String>::new());
        let again = LastPositions::open(&d).unwrap();
        assert_eq!(
            again.get(STEVE),
            Some(&LastPosition {
                name: "Steve".into(),
                dimension: "minecraft:overworld".into(),
                x: -1.0,
                y: 64.0,
                z: 2.0,
                at: NOW + 1,
            })
        );
        assert_eq!(again.find("STEVE"), Some((STEVE, "Steve".into())));
        assert_eq!(again.get(ALEX), None);
        // So are claims and homes: the lookups read them back from disk.
        let (mut c, mut p, _) = steve_world(&d);
        c.save().unwrap();
        p.save(NOW).unwrap();
        assert_eq!(smp_store::flush_dir(&d), Vec::<String>::new());
        let c = Claims::open(&d, smp_claims::Config::default()).unwrap();
        let p = Places::open(&d, homes3()).unwrap();
        let look = Lookup {
            claims: Ok(&c),
            places: Ok(&p),
            last: Ok(&again),
            find: &find,
            name_of: &name_of,
            online: &nobody_on,
        };
        assert_eq!(run(&look, 3, "smpadmin claims steve").len(), 5);
        assert_eq!(run(&look, 3, "smpadmin homes steve").len(), 3);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn only_operators_look_players_up() {
        let d = dir();
        let (c, p, l) = steve_world(&d);
        let look = Lookup {
            claims: Ok(&c),
            places: Ok(&p),
            last: Ok(&l),
            find: &find,
            name_of: &name_of,
            online: &nobody_on,
        };
        for level in [0, 2] {
            for line in [
                "smpadmin claims steve",
                "smpadmin homes steve",
                "smpadmin lastpos steve",
            ] {
                assert_eq!(
                    run(&look, level, line),
                    [Line::error("Only operators may use /smpadmin.")],
                    "level {level}: {line}"
                );
            }
        }
        assert!(command(&player(OP, 3), &look, NOW, "claims").is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn an_unknown_player_or_a_wrong_usage_is_refused() {
        let d = dir();
        let (c, p, l) = steve_world(&d);
        let look = Lookup {
            claims: Ok(&c),
            places: Ok(&p),
            last: Ok(&l),
            find: &find,
            name_of: &name_of,
            online: &nobody_on,
        };
        for what in ["claims", "homes", "lastpos"] {
            assert_eq!(
                run(&look, 3, &format!("smpadmin {what} herobrine")),
                [Line::error("No player named herobrine has played here.")]
            );
        }
        for line in [
            "smpadmin",
            "smpadmin steve",
            "smpadmin warps steve",
            "smpadmin homes",
        ] {
            let out = run(&look, 3, line);
            assert!(
                out[0].error && out[0].text.starts_with("Usage: "),
                "{line}: {out:?}"
            );
        }
        // A part whose file can't be read says so.
        let off = Lookup {
            claims: Err("claims.json can't be read".into()),
            ..look
        };
        assert_eq!(
            run(&off, 3, "smpadmin claims steve"),
            [Line::error("Claims are off: claims.json can't be read")]
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn touching_chunks_make_one_area() {
        let a = areas(&[(0, 0), (1, 0), (1, 1), (5, 5), (3, 0)]);
        assert_eq!(
            a,
            vec![vec![(0, 0), (1, 0), (1, 1)], vec![(3, 0)], vec![(5, 5)]]
        );
    }
}
