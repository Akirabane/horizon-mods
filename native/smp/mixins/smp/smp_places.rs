// Places: the server's spawn (`/spawn`, `/setspawn`), each player's homes
// (`/home`, `/sethome`, `/delhome`, `/homes`), warps (`/warp`, `/warps`,
// `/setwarp`, `/delwarp`), teleport requests between players (`/tpa`,
// `/tpaccept`, `/tpdeny`), `/rtp` (`/wild`: a random safe spot far from the
// spawn), `/back` (where the player died, or was before their last
// teleport), `/playtime` and `/rules`. A teleport waits `warmup_seconds`,
// and a player who moves or is hurt meanwhile stays where they are; the glue
// moves them with a vanilla `tp` run as the server. The number of homes grows
// with the time a player has played here, counted by the mod. Everything is
// in `places.json`, written atomically. No game types. Added to the game as
// `game::smp_places`; also a module of this crate, for its tests.

use super::smp_economy::{Out, Player};
use super::smp_store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// `config.json`'s `places` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// How long a player stands still before a teleport (0: at once).
    pub warmup_seconds: i64,
    /// How long a `/tpa` request waits for its answer.
    pub request_seconds: i64,
    /// Between two teleports of one player.
    pub cooldown_seconds: i64,
    /// Homes a player may set, by hours played here: `[hours, homes]`, the
    /// last line reached counts.
    pub homes: Vec<[i64; 2]>,
    /// `/rules`, a line each.
    pub rules: Vec<String>,
    /// `/spawn` lands on a random safe column within this many blocks of the
    /// spawn point, like vanilla's `respawn_radius` for a first join or a
    /// respawn (0: exactly the spawn point).
    pub spawn_radius: i64,
    /// `/rtp` lands at least this many blocks from the spawn,
    pub rtp_min_distance: i64,
    /// and at most this many (uniform over the ring's area, inside the
    /// world border; the Overworld only).
    pub rtp_max_distance: i64,
    /// Between two `/rtp`s of one player, kept over a restart.
    pub rtp_cooldown_seconds: i64,
    /// Chunks tried, one after another, before `/rtp` gives up.
    pub rtp_attempts: i64,
    /// How long `/rtp` waits, after the warmup, for the land to load.
    pub rtp_search_seconds: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            warmup_seconds: 3,
            request_seconds: 60,
            cooldown_seconds: 10,
            // 2 homes to start, one more every 10 hours played, up to 7.
            homes: (0..=5).map(|i| [i * 10, 2 + i]).collect(),
            rules: vec![
                "Be respectful to everyone.".into(),
                "No griefing, no stealing, no cheats.".into(),
                "The currency is earned in game only: never bought or sold for real money.".into(),
            ],
            spawn_radius: 250,
            rtp_min_distance: 500,
            rtp_max_distance: 10_000,
            rtp_cooldown_seconds: 300,
            rtp_attempts: 6,
            rtp_search_seconds: 30,
        }
    }
}

/// A place in a level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// `minecraft:overworld`, `minecraft:the_nether`, `minecraft:the_end`.
    pub dimension: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A player on the server, as the places see them.
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub uuid: u128,
    pub name: String,
    pub at: Place,
    pub health: f32,
}

/// `places.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    spawn: Option<Place>,
    warps: BTreeMap<String, Place>,
    /// Homes by player UUID, then by name.
    homes: BTreeMap<String, BTreeMap<String, Place>>,
    /// Seconds played here, by player UUID.
    played: BTreeMap<String, i64>,
    /// The last `/rtp` (ms), by player UUID.
    rtp: BTreeMap<String, i64>,
    /// The Overworld chunks an older SMP's `/rtp` held with `forceload add`
    /// and hadn't let go of when the server stopped: let go once at the next
    /// start. `/rtp` holds its chunks with tickets now, never saved.
    forced: BTreeSet<(i32, i32)>,
}

/// The world border, `[min_x, min_z, max_x, max_z]`.
pub type Bounds = [f64; 4];

/// The Overworld, the only level `/rtp` goes to.
pub const OVERWORLD: &str = "minecraft:overworld";

/// An `/rtp` looking for its spot.
#[derive(Clone, Debug, PartialEq)]
pub struct Rtp {
    /// Where the distances count from.
    centre: (f64, f64),
    border: Option<Bounds>,
    /// The chunks left to try, the one loading first.
    chunks: Vec<(i32, i32)>,
    /// The spot found, waiting for the warmup's end.
    spot: Option<Place>,
    /// "Looking for a safe spot" was said.
    told: bool,
}

/// A loaded chunk's columns, as the game read them: each column's `x`,
/// `z`, height (the first block above its ground) and its ground, feet and
/// head blocks, as indexes into `palette`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ground {
    pub palette: Vec<String>,
    pub columns: Vec<(i32, i32, i32, [u16; 3])>,
}

/// Where a teleport goes.
#[derive(Clone, Debug, PartialEq)]
enum To {
    /// A place, and what it is (`home "base"`).
    Place(Place, String),
    /// The spawn point: `/spawn` fuzzes it by `spawn_radius`.
    Spawn(Place),
    /// A player, wherever they are when the warmup ends.
    Player(u128, String),
    /// `/rtp`: a safe spot found in the chunks it loads.
    Random(Rtp),
}

/// A teleport waiting for its warmup.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub uuid: u128,
    to: To,
    from: Place,
    health: f32,
    due: i64,
}

/// A `/tpa` request: `from` asks to go to `to`.
struct Request {
    from: u128,
    from_name: String,
    to: u128,
    at: i64,
}

/// A teleport to make now: the player's name and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Teleport {
    pub uuid: u128,
    pub name: String,
    pub to: Place,
    /// What the player is told once there.
    pub done: String,
    /// Where to, in words (`where you died`).
    pub what: String,
    /// Land on a safe column within this many blocks of `to` instead of on it
    /// (0: on it). Only `/spawn` sets it.
    pub spread: i64,
    /// What it changed: `call_off` puts it back.
    undo: Undo,
}

/// The cooldown and `/back` before a teleport, and the `/rtp` cooldown
/// when it is one.
#[derive(Clone, Debug, PartialEq)]
struct Undo {
    last: Option<i64>,
    back: Option<(Place, bool)>,
    rtp: Option<Option<i64>>,
}

impl Teleport {
    /// The command that moves the player from the point `to` to a safe column
    /// within `spread` blocks of it: vanilla's `spreadplayers` (a column that
    /// is not fluid, fire or magma and has solid ground, found by retrying),
    /// the check behind `respawn_radius`. None when `spread` is 0.
    pub fn spread_line(&self) -> Option<String> {
        (self.spread >= 1).then(|| {
            format!(
                "execute in {} run spreadplayers {:.3} {:.3} 0 {} false {}",
                self.to.dimension, self.to.x, self.to.z, self.spread, self.name
            )
        })
    }
}

/// The places' commands.
pub const COMMANDS: [&str; 18] = [
    "spawn", "setspawn", "home", "sethome", "delhome", "homes", "warp", "warps", "setwarp",
    "delwarp", "tpa", "tpaccept", "tpdeny", "rules", "back", "playtime", "rtp", "wild",
];

pub struct Places {
    pub config: Config,
    path: PathBuf,
    data: Data,
    /// Homes, warps or the spawn changed: written at the next save.
    changed: bool,
    /// Time played changed: written once a minute.
    played_at: Option<i64>,
    pub pending: Vec<Pending>,
    requests: Vec<Request>,
    last: HashMap<u128, i64>,
    /// Where `/back` goes: the place and whether they died there. Not kept
    /// over a restart.
    back: HashMap<u128, (Place, bool)>,
    /// The players seen dead (health 0) and not respawned yet.
    dead: HashSet<u128>,
    /// Chunks an `/rtp` landed in, held loaded until then (ms): the
    /// player's own view takes over.
    release: Vec<((i32, i32), i64)>,
    /// The chunks `/rtp` holds with its ticket (`smp:rtp`) now.
    held: BTreeSet<(i32, i32)>,
    /// `/rtp`'s random (never the world's).
    pub rng: u64,
    /// The world border, as the game last told (None: the default one).
    pub border: Option<Bounds>,
}

/// `2 h 05 min`, `12 min`, `40 s` for the time left on a cooldown.
fn wait_text(ms: i64) -> String {
    let s = (ms + 999) / 1000;
    if s < 60 {
        format!("{s} s")
    } else {
        duration(s + 59)
    }
}

/// SplitMix64: the next of `state`'s numbers.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A number in `0..1`.
fn unit(state: &mut u64) -> f64 {
    (next(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// A point `min..=max` blocks from `centre`, uniform over the ring's area,
/// from two numbers in `0..1`.
pub fn ring_point(centre: (f64, f64), min: f64, max: f64, u: f64, v: f64) -> (f64, f64) {
    let r = (min * min + u * (max * max - min * min)).sqrt();
    let a = v * std::f64::consts::TAU;
    (centre.0 + r * a.cos(), centre.1 + r * a.sin())
}

/// Whether `(x, z)` is at least 16 blocks inside the border.
fn inside(border: Option<Bounds>, x: f64, z: f64) -> bool {
    border.is_none_or(|[x0, z0, x1, z1]| {
        x >= x0 + 16.0 && x <= x1 - 16.0 && z >= z0 + 16.0 && z <= z1 - 16.0
    })
}

/// A block a player may stand on: solid, no fluid, nothing that hurts.
fn ground_ok(name: &str) -> bool {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    !(matches!(
        n,
        "air"
            | "cave_air"
            | "void_air"
            | "water"
            | "lava"
            | "fire"
            | "soul_fire"
            | "cactus"
            | "magma_block"
            | "powder_snow"
            | "campfire"
            | "soul_campfire"
            | "sweet_berry_bush"
            | "pointed_dripstone"
            | "wither_rose"
            | "bubble_column"
            | "kelp"
            | "kelp_plant"
            | "seagrass"
            | "tall_seagrass"
            | "bedrock"
    ) || n.ends_with("_leaves"))
}

/// A block a player's feet or head may be in: air, or a plant they walk through.
fn room_ok(name: &str) -> bool {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    matches!(
        n,
        "air" | "cave_air" | "short_grass" | "tall_grass" | "fern" | "large_fern" | "snow"
    )
}

/// `2 h 05 min`, `12 min`.
fn duration(seconds: i64) -> String {
    let (h, m) = (seconds / 3600, seconds % 3600 / 60);
    if h > 0 {
        format!("{h} h {m:02} min")
    } else {
        format!("{m} min")
    }
}

/// A home's or a warp's name: 1 to 16 of `a-z 0-9 _ -`.
fn name_ok(name: &str) -> bool {
    (1..=16).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-".contains(c))
}

fn moved(a: &Place, b: &Place) -> bool {
    let d = (a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2);
    a.dimension != b.dimension || d > 0.25
}

impl Places {
    /// `places.json` in `dir`; an error when it exists but can't be read
    /// (never replaced by an empty one).
    pub fn open(dir: &Path, config: Config) -> Result<Self, String> {
        let path = dir.join("places.json");
        let data = smp_store::read_json(&path)?;
        Ok(Self {
            config,
            path,
            data,
            changed: false,
            played_at: None,
            pending: Vec::new(),
            requests: Vec::new(),
            last: HashMap::new(),
            back: HashMap::new(),
            dead: Default::default(),
            release: Vec::new(),
            held: BTreeSet::new(),
            rng: {
                use std::hash::BuildHasher;
                std::collections::hash_map::RandomState::new()
                    .hash_one(std::time::SystemTime::now())
            },
            border: None,
        })
    }

    /// Writes `places.json` when homes, warps or the spawn changed, or once
    /// a minute when only the time played did.
    pub fn save(&mut self, now: i64) -> Result<(), String> {
        let played = self.played_at.is_some_and(|t| now - t >= 60_000);
        if !self.changed && !played {
            return Ok(());
        }
        smp_store::save_json(&self.path, &self.data)?;
        self.changed = false;
        self.played_at = None;
        Ok(())
    }

    /// Counts a second played for each of these players.
    pub fn played(&mut self, uuids: &[u128], now: i64) {
        for &u in uuids {
            *self
                .data
                .played
                .entry(smp_store::uuid_string(u))
                .or_default() += 1;
        }
        if !uuids.is_empty() {
            self.played_at.get_or_insert(now);
        }
    }

    /// The seconds a player has played here.
    pub fn played_seconds(&self, uuid: u128) -> i64 {
        self.data
            .played
            .get(&smp_store::uuid_string(uuid))
            .copied()
            .unwrap_or(0)
    }

    /// A player's homes, by name.
    pub fn homes_of(&self, uuid: u128) -> Vec<(String, Place)> {
        self.data
            .homes
            .get(&smp_store::uuid_string(uuid))
            .into_iter()
            .flatten()
            .map(|(n, p)| (n.clone(), p.clone()))
            .collect()
    }

    /// How many homes a player may have now.
    pub fn home_limit(&self, uuid: u128) -> usize {
        let hours = self
            .data
            .played
            .get(&smp_store::uuid_string(uuid))
            .copied()
            .unwrap_or(0)
            / 3600;
        self.config
            .homes
            .iter()
            .filter(|[h, _]| hours >= *h)
            .map(|[_, n]| (*n).max(0) as usize)
            .next_back()
            .unwrap_or(0)
    }

    /// A teleport the game couldn't make: no
    /// cooldown, and `/back` goes where it went before.
    pub fn call_off(&mut self, t: &Teleport) {
        if let Some(rtp) = t.undo.rtp {
            let key = smp_store::uuid_string(t.uuid);
            match rtp {
                Some(at) => self.data.rtp.insert(key, at),
                None => self.data.rtp.remove(&key),
            };
            self.changed = true;
        }
        match t.undo.last {
            Some(at) => self.last.insert(t.uuid, at),
            None => self.last.remove(&t.uuid),
        };
        match &t.undo.back {
            Some(b) => self.back.insert(t.uuid, b.clone()),
            None => self.back.remove(&t.uuid),
        };
    }

    /// Starts a teleport's warmup, or says why not.
    fn start(&mut self, me: &Body, to: To, now: i64) -> Vec<Out> {
        if let Some(&t) = self.last.get(&me.uuid) {
            let wait = t + self.config.cooldown_seconds * 1000 - now;
            if wait > 0 {
                return vec![Out::Error(format!(
                    "Wait {} s before your next teleport.",
                    (wait + 999) / 1000
                ))];
            }
        }
        self.pending.retain(|p| p.uuid != me.uuid);
        self.pending.push(Pending {
            uuid: me.uuid,
            to,
            from: me.at.clone(),
            health: me.health,
            due: now + self.config.warmup_seconds.max(0) * 1000,
        });
        if self.config.warmup_seconds > 0 {
            vec![Out::Reply(format!(
                "Teleporting in {} s: don't move.",
                self.config.warmup_seconds
            ))]
        } else {
            Vec::new()
        }
    }

    /// `/rtp`: refused outside the Overworld and during its cooldown;
    /// otherwise the chunks to try are drawn and the warmup starts. The
    /// distances count from the spawn (`/setspawn`), 0 0 without one.
    fn rtp(&mut self, me: &Body, now: i64) -> Vec<Out> {
        if me.at.dimension != OVERWORLD {
            return vec![Out::Error("/rtp only works in the Overworld.".into())];
        }
        let key = smp_store::uuid_string(me.uuid);
        if let Some(&t) = self.data.rtp.get(&key) {
            let wait = t + self.config.rtp_cooldown_seconds * 1000 - now;
            if wait > 0 {
                return vec![Out::Error(format!(
                    "Wait {} before your next /rtp.",
                    wait_text(wait)
                ))];
            }
        }
        let centre = match &self.data.spawn {
            Some(s) if s.dimension == OVERWORLD => (s.x, s.z),
            _ => (0.0, 0.0),
        };
        let chunks = self.rtp_chunks(centre, self.border);
        if chunks.is_empty() {
            return vec![Out::Error(
                "No land for /rtp: the world border is too close to the spawn.".into(),
            )];
        }
        let border = self.border;
        self.start(
            me,
            To::Random(Rtp {
                centre,
                border,
                chunks,
                spot: None,
                told: false,
            }),
            now,
        )
    }

    /// Up to `rtp_attempts` distinct chunks around points drawn on the ring,
    /// inside the border (none when the border leaves no room).
    fn rtp_chunks(&mut self, centre: (f64, f64), border: Option<Bounds>) -> Vec<(i32, i32)> {
        let min = self.config.rtp_min_distance.max(0) as f64;
        let max = (self.config.rtp_max_distance as f64).max(min);
        let mut chunks = Vec::new();
        // Drawn until enough fall inside the border: a border that cuts the
        // ring takes more draws (a few microseconds each).
        for _ in 0..2_000 {
            if chunks.len() >= self.config.rtp_attempts.max(1) as usize {
                break;
            }
            let (u, v) = (unit(&mut self.rng), unit(&mut self.rng));
            let (x, z) = ring_point(centre, min, max, u, v);
            let c = ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4);
            if inside(border, x, z) && !chunks.contains(&c) {
                chunks.push(c);
            }
        }
        chunks
    }

    /// The safe columns of a loaded chunk that are within the ring and the
    /// border, and one of them at random.
    fn pick(&mut self, rtp: &Rtp, ground: &Ground) -> Option<Place> {
        let ok: Vec<bool> = ground.palette.iter().map(|n| ground_ok(n)).collect();
        let room: Vec<bool> = ground.palette.iter().map(|n| room_ok(n)).collect();
        let (min, max) = (
            self.config.rtp_min_distance.max(0) as f64,
            self.config.rtp_max_distance as f64,
        );
        let safe: Vec<&(i32, i32, i32, [u16; 3])> = ground
            .columns
            .iter()
            .filter(|(x, z, _, [g, f, h])| {
                let (px, pz) = (f64::from(*x) + 0.5, f64::from(*z) + 0.5);
                let d = (px - rtp.centre.0).hypot(pz - rtp.centre.1);
                let at = |i: &u16, v: &[bool]| v.get(usize::from(*i)).copied().unwrap_or(false);
                (min..=max).contains(&d)
                    && inside(rtp.border, px, pz)
                    && at(g, &ok)
                    && at(f, &room)
                    && at(h, &room)
            })
            .collect();
        if safe.is_empty() {
            return None;
        }
        let &(x, z, y, _) = safe[(next(&mut self.rng) % safe.len() as u64) as usize];
        Some(Place {
            dimension: OVERWORLD.into(),
            x: f64::from(x) + 0.5,
            y: f64::from(y),
            z: f64::from(z) + 0.5,
        })
    }

    /// Whether an `/rtp` waits for land: the game reads it then
    /// (`rtp_look`).
    pub fn rtp_searching(&self) -> bool {
        self.pending
            .iter()
            .any(|p| matches!(&p.to, To::Random(r) if r.spot.is_none() && !r.chunks.is_empty()))
    }

    /// Each `/rtp` still looking reads the chunk it loads: None while it
    /// isn't loaded yet; with no safe column, the next chunk is tried.
    pub fn rtp_look(&mut self, look: &mut dyn FnMut(u128, (i32, i32)) -> Option<Ground>) {
        let mut pending = std::mem::take(&mut self.pending);
        for p in &mut pending {
            let To::Random(r) = &mut p.to else { continue };
            if r.spot.is_some() {
                continue;
            }
            let Some(&c) = r.chunks.first() else { continue };
            let Some(ground) = look(p.uuid, c) else {
                continue;
            };
            match self.pick(r, &ground) {
                Some(spot) => r.spot = Some(spot),
                None => {
                    r.chunks.remove(0);
                }
            }
        }
        self.pending = pending;
    }

    /// The chunks `/rtp` wants loaded now: each search's current one, and
    /// those just landed in.
    pub fn rtp_wanted(&mut self, now: i64) -> BTreeSet<(i32, i32)> {
        self.release.retain(|&(_, until)| until > now);
        let mut wanted: BTreeSet<(i32, i32)> = self.release.iter().map(|&(c, _)| c).collect();
        for p in &self.pending {
            if let To::Random(r) = &p.to
                && let Some(&c) = r.chunks.first()
            {
                wanted.insert(c);
            }
        }
        wanted
    }

    /// Holds the chunks `/rtp` wants with a chunk ticket (`hold(chunk,
    /// true)`, true when the game took it) and lets go of the others it holds
    /// (`hold(chunk, false)`). A ticket never waits for its chunk: the search
    /// reads it once it is loaded (`rtp_look`). `/forceload add` did wait,
    /// on the server thread, until the chunk was generated (vanilla's
    /// `setChunkForced` loads it), and froze the server for every `/rtp`.
    /// Returns the commands that let go, once, of the chunks an older SMP
    /// forced and saved.
    pub fn force(
        &mut self,
        now: i64,
        hold: &mut dyn FnMut((i32, i32), bool) -> bool,
    ) -> Vec<String> {
        let mut run = Vec::new();
        if !self.data.forced.is_empty() {
            for (x, z) in std::mem::take(&mut self.data.forced) {
                run.push(format!(
                    "execute in {OVERWORLD} run forceload remove {} {}",
                    x * 16,
                    z * 16
                ));
            }
            self.changed = true;
        }
        let wanted = self.rtp_wanted(now);
        for &c in &wanted {
            if !self.held.contains(&c) && hold(c, true) {
                self.held.insert(c);
            }
        }
        let gone: Vec<(i32, i32)> = self.held.difference(&wanted).copied().collect();
        for c in gone {
            hold(c, false);
            self.held.remove(&c);
        }
        run
    }

    /// The places' commands; None for another command. `bodies` are the
    /// players on, `me` among them.
    pub fn command(
        &mut self,
        me: &Player,
        bodies: &[Body],
        now: i64,
        line: &str,
    ) -> Option<Vec<Out>> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let command = *words.first()?;
        if !COMMANDS.contains(&command) {
            return None;
        }
        let Some(body) = bodies.iter().find(|b| b.uuid == me.uuid) else {
            return Some(vec![Out::Error("You are not in the world.".into())]);
        };
        let key = smp_store::uuid_string(me.uuid);
        let named = |n: &str| bodies.iter().find(|b| b.name.eq_ignore_ascii_case(n));
        Some(match (command, &words[1..]) {
            ("back", []) => match self.back.get(&me.uuid).cloned() {
                Some((p, died)) => {
                    let what = if died {
                        "where you died"
                    } else {
                        "where you were"
                    };
                    self.start(body, To::Place(p, what.into()), now)
                }
                None => vec![Out::Error(
                    "Nowhere to go back to yet: /back follows a death or a teleport.".into(),
                )],
            },
            ("playtime", []) => vec![Out::Reply(format!(
                "You have played here for {}.",
                duration(self.played_seconds(me.uuid))
            ))],
            ("playtime", [name]) => match named(name) {
                Some(b) => vec![Out::Reply(format!(
                    "{} has played here for {}.",
                    b.name,
                    duration(self.played_seconds(b.uuid))
                ))],
                None => vec![Out::Error(format!("{name} isn't on."))],
            },
            ("rtp" | "wild", []) => self.rtp(body, now),
            ("rules", []) => {
                let mut out = vec![Out::Reply("Rules of this server:".into())];
                for (i, r) in self.config.rules.iter().enumerate() {
                    out.push(Out::Reply(format!("{}. {r}", i + 1)));
                }
                out
            }
            ("spawn", []) => match self.data.spawn.clone() {
                Some(p) => self.start(body, To::Spawn(p), now),
                None => vec![Out::Error(
                    "No spawn yet: an operator sets it with /setspawn.".into(),
                )],
            },
            ("setspawn", []) if me.level >= 3 => {
                self.data.spawn = Some(body.at.clone());
                self.changed = true;
                vec![Out::Reply("The spawn is here now.".into())]
            }
            ("homes", []) => {
                let homes = self.data.homes.get(&key);
                let names: Vec<&str> = homes
                    .into_iter()
                    .flat_map(|h| h.keys())
                    .map(String::as_str)
                    .collect();
                vec![Out::Reply(format!(
                    "Your homes ({} of {}): {}",
                    names.len(),
                    self.home_limit(me.uuid),
                    if names.is_empty() {
                        "none yet, /sethome [name]".into()
                    } else {
                        names.join(", ")
                    }
                ))]
            }
            ("home", rest) | ("sethome", rest) | ("delhome", rest) if rest.len() <= 1 => {
                let name = rest
                    .first()
                    .map_or("home".to_owned(), |n| n.to_ascii_lowercase());
                if !name_ok(&name) {
                    return Some(vec![Out::Error(
                        "A home's name: up to 16 of a-z, 0-9, _ and -.".into(),
                    )]);
                }
                let homes = self.data.homes.get(&key);
                let has = homes.is_some_and(|h| h.contains_key(&name));
                match command {
                    "home" => match homes.and_then(|h| h.get(&name)).cloned() {
                        Some(p) => self.start(body, To::Place(p, format!("home \"{name}\"")), now),
                        None => vec![Out::Error(format!(
                            "No home \"{name}\": /homes lists yours."
                        ))],
                    },
                    "sethome" => {
                        let count = homes.map_or(0, |h| h.len());
                        let limit = self.home_limit(me.uuid);
                        if !has && count >= limit {
                            vec![Out::Error(format!(
                                "You have {count} of {limit} homes: play longer for more, or move one (/sethome <name> over it)."
                            ))]
                        } else {
                            self.data
                                .homes
                                .entry(key)
                                .or_default()
                                .insert(name.clone(), body.at.clone());
                            self.changed = true;
                            vec![Out::Reply(format!("Home \"{name}\" set here."))]
                        }
                    }
                    _ if has => {
                        if let Some(h) = self.data.homes.get_mut(&key) {
                            h.remove(&name);
                            if h.is_empty() {
                                self.data.homes.remove(&key);
                            }
                        }
                        self.changed = true;
                        vec![Out::Reply(format!("Home \"{name}\" deleted."))]
                    }
                    _ => vec![Out::Error(format!("No home \"{name}\"."))],
                }
            }
            ("warps", []) => {
                let names: Vec<&str> = self.data.warps.keys().map(String::as_str).collect();
                vec![Out::Reply(if names.is_empty() {
                    "No warps yet.".into()
                } else {
                    format!("Warps: {}", names.join(", "))
                })]
            }
            ("warp", [name]) => match self.data.warps.get(&name.to_ascii_lowercase()).cloned() {
                Some(p) => self.start(
                    body,
                    To::Place(p, format!("warp \"{}\"", name.to_ascii_lowercase())),
                    now,
                ),
                None => vec![Out::Error(format!(
                    "No warp \"{name}\": /warps lists them."
                ))],
            },
            ("setwarp", [name]) if me.level >= 3 => {
                let name = name.to_ascii_lowercase();
                if !name_ok(&name) {
                    return Some(vec![Out::Error(
                        "A warp's name: up to 16 of a-z, 0-9, _ and -.".into(),
                    )]);
                }
                self.data.warps.insert(name.clone(), body.at.clone());
                self.changed = true;
                vec![Out::Reply(format!("Warp \"{name}\" set here."))]
            }
            ("delwarp", [name]) if me.level >= 3 => {
                if self.data.warps.remove(&name.to_ascii_lowercase()).is_some() {
                    self.changed = true;
                    vec![Out::Reply(format!("Warp \"{name}\" deleted."))]
                } else {
                    vec![Out::Error(format!("No warp \"{name}\"."))]
                }
            }
            ("tpa", [name]) => match named(name) {
                None => vec![Out::Error(format!("{name} is not on."))],
                Some(t) if t.uuid == me.uuid => vec![Out::Error("That's you.".into())],
                Some(t) => {
                    self.requests.retain(|r| r.from != me.uuid);
                    self.requests.push(Request {
                        from: me.uuid,
                        from_name: me.name.clone(),
                        to: t.uuid,
                        at: now,
                    });
                    vec![
                        Out::Reply(format!(
                            "Asked {} to teleport to them ({} s to answer).",
                            t.name, self.config.request_seconds
                        )),
                        Out::Tell(
                            t.uuid,
                            format!(
                                "{} asks to teleport to you: /tpaccept {0} or /tpdeny {0}.",
                                me.name
                            ),
                        ),
                    ]
                }
            },
            ("tpaccept", rest) | ("tpdeny", rest) if rest.len() <= 1 => {
                let found = self.requests.iter().rposition(|r| {
                    r.to == me.uuid
                        && rest
                            .first()
                            .is_none_or(|n| r.from_name.eq_ignore_ascii_case(n))
                });
                let Some(i) = found else {
                    return Some(vec![Out::Error("No teleport request to answer.".into())]);
                };
                let r = self.requests.remove(i);
                if command == "tpdeny" {
                    return Some(vec![
                        Out::Reply(format!("You turned down {}'s request.", r.from_name)),
                        Out::Tell(r.from, format!("{} turned down your request.", me.name)),
                    ]);
                }
                let Some(from) = bodies.iter().find(|b| b.uuid == r.from) else {
                    return Some(vec![Out::Error(format!("{} is gone.", r.from_name))]);
                };
                let mut out = vec![Out::Reply(format!("{} is on the way.", r.from_name))];
                let start = self.start(from, To::Player(me.uuid, me.name.clone()), now);
                // The warmup's lines are for the one who moves.
                out.extend(start.into_iter().map(|o| match o {
                    Out::Reply(s) | Out::Error(s) => Out::Tell(r.from, s),
                    o => o,
                }));
                out
            }
            _ => vec![Out::Error(
                match command {
                    "home" | "sethome" | "delhome" => {
                        "Usage: /home, /sethome, /delhome [name]; /homes"
                    }
                    "warp" | "setwarp" | "delwarp" => "Usage: /warp <name>; /warps",
                    "tpa" => "Usage: /tpa <player>",
                    "tpaccept" | "tpdeny" => "Usage: /tpaccept [player], /tpdeny [player]",
                    "rtp" | "wild" => "Usage: /rtp",
                    _ => "You may not use this command.",
                }
                .into(),
            )],
        })
    }

    /// Each tick: warmups that ended become teleports, those of players who
    /// moved, were hurt or left are called off; requests nobody answered
    /// expire.
    pub fn tick(&mut self, bodies: &[Body], now: i64) -> (Vec<Out>, Vec<Teleport>) {
        let mut out = Vec::new();
        let mut go = Vec::new();
        let wait = self.config.request_seconds * 1000;
        self.requests.retain(|r| {
            let keep = now - r.at < wait;
            if !keep {
                out.push(Out::Tell(r.from, "Your teleport request expired.".into()));
            }
            keep
        });
        // A death, seen once a second: /back goes there.
        for b in bodies {
            if b.health <= 0.0 {
                if self.dead.insert(b.uuid) {
                    self.back.insert(b.uuid, (b.at.clone(), true));
                }
            } else {
                self.dead.remove(&b.uuid);
            }
        }
        for p in std::mem::take(&mut self.pending) {
            let Some(me) = bodies.iter().find(|b| b.uuid == p.uuid) else {
                continue;
            };
            if moved(&p.from, &me.at) {
                out.push(Out::Tell(p.uuid, "Teleport called off: you moved.".into()));
            } else if me.health < p.health || me.health <= 0.0 {
                out.push(Out::Tell(
                    p.uuid,
                    "Teleport called off: you were hurt.".into(),
                ));
            } else if now < p.due {
                self.pending.push(p);
            } else if let To::Random(r) = &p.to
                && r.spot.is_none()
            {
                // `/rtp` after the warmup, its land not found yet.
                let search = self.config.rtp_search_seconds.max(0) * 1000;
                if r.chunks.is_empty() {
                    out.push(Out::Tell(
                        p.uuid,
                        "No safe spot found for /rtp: try again.".into(),
                    ));
                } else if now - p.due > search {
                    out.push(Out::Tell(
                        p.uuid,
                        "/rtp couldn't load the land in time: try again.".into(),
                    ));
                } else {
                    let mut p = p;
                    if let To::Random(r) = &mut p.to
                        && !r.told
                    {
                        r.told = true;
                        out.push(Out::Tell(
                            p.uuid,
                            "Looking for a safe spot: don't move.".into(),
                        ));
                    }
                    self.pending.push(p);
                }
            } else {
                let mut spread = 0;
                let mut rtp = None;
                let (to, what) = match &p.to {
                    To::Random(r) => {
                        let spot = r.spot.clone();
                        if let (Some(s), Some(&c)) = (&spot, r.chunks.first()) {
                            let d = (s.x - r.centre.0).hypot(s.z - r.centre.1).round();
                            rtp = Some((c, d as i64));
                        }
                        (spot, "a random spot".to_owned())
                    }
                    To::Place(place, what) => (Some(place.clone()), what.clone()),
                    To::Spawn(place) => {
                        spread = self.config.spawn_radius.max(0);
                        (Some(place.clone()), "the spawn".to_owned())
                    }
                    To::Player(u, name) => (
                        bodies.iter().find(|b| b.uuid == *u).map(|b| b.at.clone()),
                        name.clone(),
                    ),
                };
                match to {
                    Some(to) => {
                        let mut done = format!("Teleported to {what}.");
                        let mut undo_rtp = None;
                        if let Some((chunk, d)) = rtp {
                            undo_rtp =
                                Some(self.data.rtp.insert(smp_store::uuid_string(p.uuid), now));
                            self.changed = true;
                            // Held a little longer: the player's own view takes it over.
                            self.release.push((chunk, now + 10_000));
                            done = format!(
                                "Teleported to a random spot {d} blocks from the spawn ({:.0} {:.0} {:.0}).",
                                to.x.floor(),
                                to.y,
                                to.z.floor()
                            );
                        }
                        let undo = Undo {
                            last: self.last.insert(p.uuid, now),
                            back: self.back.insert(p.uuid, (p.from.clone(), false)),
                            rtp: undo_rtp,
                        };
                        go.push(Teleport {
                            uuid: p.uuid,
                            name: me.name.clone(),
                            to,
                            done,
                            what,
                            spread,
                            undo,
                        });
                    }
                    None => out.push(Out::Tell(
                        p.uuid,
                        format!("Teleport called off: {what} is gone."),
                    )),
                }
            }
        }
        (out, go)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const A: u128 = 1;
    const B: u128 = 2;

    fn dir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("smp-places-{}-{n}", std::process::id()))
    }
    fn place(x: f64) -> Place {
        Place {
            dimension: "minecraft:overworld".into(),
            x,
            y: 64.0,
            z: 0.0,
        }
    }
    fn body(uuid: u128, x: f64) -> Body {
        Body {
            uuid,
            name: if uuid == A { "Alex" } else { "Steve" }.into(),
            at: place(x),
            health: 20.0,
        }
    }
    fn player(uuid: u128, level: u8) -> Player {
        Player {
            uuid,
            name: if uuid == A { "Alex" } else { "Steve" }.into(),
            level,
        }
    }

    #[test]
    fn a_home_is_reached_after_the_warmup_unless_the_player_moves() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        let here = [body(A, 10.0)];
        let out = p.command(&player(A, 0), &here, 0, "sethome base").unwrap();
        assert_eq!(out, vec![Out::Reply("Home \"base\" set here.".into())]);
        let there = [body(A, 500.0)];
        p.command(&player(A, 0), &there, 0, "home base").unwrap();
        assert!(p.tick(&there, 2_999).1.is_empty(), "the warmup");
        let (_, go) = p.tick(&there, 3_000);
        assert_eq!(go[0].to, place(10.0));
        assert_eq!(go[0].name, "Alex");
        // The cooldown, then a move calls the next one off.
        let out = p
            .command(&player(A, 0), &there, 5_000, "home base")
            .unwrap();
        assert!(
            matches!(&out[0], Out::Error(s) if s.contains("Wait")),
            "{out:?}"
        );
        p.command(&player(A, 0), &there, 13_000, "home base")
            .unwrap();
        let (out, go) = p.tick(&[body(A, 501.0)], 14_000);
        assert!(go.is_empty());
        assert_eq!(
            out,
            vec![Out::Tell(A, "Teleport called off: you moved.".into())]
        );
        // Saved and read back.
        p.save(0).unwrap();
        let again = Places::open(&d, Config::default()).unwrap();
        assert_eq!(
            again.data.homes[&smp_store::uuid_string(A)]["base"],
            place(10.0)
        );
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn back_goes_where_the_player_died_or_was_before_a_teleport() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        let out = p
            .command(&player(A, 0), &[body(A, 0.0)], 0, "back")
            .unwrap();
        assert!(matches!(&out[0], Out::Error(s) if s.contains("Nowhere")));
        // Dead at x 40: seen by the next tick, once.
        let mut dead = body(A, 40.0);
        dead.health = 0.0;
        p.tick(&[dead.clone()], 1_000);
        dead.at = place(41.0);
        p.tick(&[dead], 2_000);
        let alive = [body(A, 0.0)];
        p.tick(&alive, 3_000);
        p.command(&player(A, 0), &alive, 3_000, "back").unwrap();
        let (_, go) = p.tick(&alive, 6_000);
        assert_eq!(go[0].to, place(40.0));
        assert_eq!(go[0].done, "Teleported to where you died.");
        // Then /back goes to where that teleport started.
        let there = [body(A, 40.0)];
        p.command(&player(A, 0), &there, 20_000, "back").unwrap();
        let (_, go) = p.tick(&there, 23_000);
        assert_eq!(go[0].to, place(0.0));
        assert_eq!(go[0].done, "Teleported to where you were.");
        p.played(&[A], 0);
        let out = p.command(&player(A, 0), &there, 0, "playtime").unwrap();
        assert_eq!(
            out,
            vec![Out::Reply("You have played here for 0 min.".into())]
        );
        assert_eq!(duration(7_500), "2 h 05 min");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn homes_grow_with_time_played() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        let here = [body(A, 0.0)];
        p.command(&player(A, 0), &here, 0, "sethome").unwrap();
        p.command(&player(A, 0), &here, 0, "sethome two").unwrap();
        let out = p.command(&player(A, 0), &here, 0, "sethome three").unwrap();
        assert!(
            matches!(&out[0], Out::Error(s) if s.contains("2 of 2")),
            "{out:?}"
        );
        // Moving a home is always allowed.
        let out = p.command(&player(A, 0), &here, 0, "sethome home").unwrap();
        assert!(matches!(&out[0], Out::Reply(_)));
        p.save(0).unwrap();
        for _ in 0..10 * 3600 {
            p.played(&[A], 0);
        }
        assert_eq!(p.home_limit(A), 3);
        // Time played is written once a minute.
        p.save(59_999).unwrap();
        assert_eq!(
            Places::open(&d, Config::default()).unwrap().home_limit(A),
            2
        );
        p.save(60_000).unwrap();
        let out = p.command(&player(A, 0), &here, 0, "sethome three").unwrap();
        assert!(matches!(&out[0], Out::Reply(_)), "{out:?}");
        let again = Places::open(&d, Config::default()).unwrap();
        assert_eq!(again.home_limit(A), 3);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_request_moves_the_asker_to_the_one_who_accepts() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        let both = [body(A, 0.0), body(B, 100.0)];
        let out = p.command(&player(A, 0), &both, 0, "tpa steve").unwrap();
        assert!(
            matches!(&out[1], Out::Tell(B, s) if s.contains("/tpaccept Alex")),
            "{out:?}"
        );
        let out = p.command(&player(B, 0), &both, 1_000, "tpaccept").unwrap();
        assert_eq!(
            out[1],
            Out::Tell(A, "Teleporting in 3 s: don't move.".into())
        );
        // Steve walked on meanwhile: Alex lands where Steve is then.
        let later = [body(A, 0.0), body(B, 120.0)];
        let (_, go) = p.tick(&later, 4_000);
        assert_eq!((go[0].uuid, go[0].to.x), (A, 120.0));
        // A hurt player stays; an unanswered request expires.
        p.command(&player(A, 0), &later, 20_000, "tpa steve")
            .unwrap();
        p.command(&player(B, 0), &later, 20_000, "tpaccept alex")
            .unwrap();
        let mut hurt = later.clone();
        hurt[0].health = 15.0;
        let (out, go) = p.tick(&hurt, 21_000);
        assert!(go.is_empty());
        assert_eq!(
            out,
            vec![Out::Tell(A, "Teleport called off: you were hurt.".into())]
        );
        p.command(&player(A, 0), &later, 30_000, "tpa steve")
            .unwrap();
        let (out, _) = p.tick(&later, 90_000);
        assert_eq!(
            out,
            vec![Out::Tell(A, "Your teleport request expired.".into())]
        );
        let out = p.command(&player(B, 0), &later, 90_000, "tpdeny").unwrap();
        assert!(matches!(&out[0], Out::Error(_)));
    }

    #[test]
    fn a_called_off_teleport_keeps_back_and_no_cooldown() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        let mut dead = body(A, 40.0);
        dead.health = 0.0;
        p.tick(&[dead], 0);
        let alive = [body(A, 0.0)];
        let out = p.command(&player(A, 0), &alive, 1_000, "back").unwrap();
        assert_eq!(
            out,
            vec![Out::Reply("Teleporting in 3 s: don't move.".into())]
        );
        let (_, go) = p.tick(&alive, 4_000);
        assert_eq!(go[0].what, "where you died");
        // The game couldn't make it: /back still goes where they died, at once.
        p.call_off(&go[0]);
        p.command(&player(A, 0), &alive, 4_000, "back").unwrap();
        let (_, go) = p.tick(&alive, 7_000);
        assert_eq!(go[0].to, place(40.0));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn spawn_and_warps_are_set_by_operators() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        let here = [body(A, 7.0)];
        let out = p.command(&player(A, 0), &here, 0, "spawn").unwrap();
        assert!(matches!(&out[0], Out::Error(s) if s.contains("/setspawn")));
        let out = p.command(&player(A, 0), &here, 0, "setspawn").unwrap();
        assert!(matches!(&out[0], Out::Error(_)), "level 0");
        p.command(&player(A, 3), &here, 0, "setspawn").unwrap();
        p.command(&player(A, 3), &here, 0, "setwarp Shop").unwrap();
        let out = p.command(&player(A, 0), &here, 0, "warps").unwrap();
        assert_eq!(out, vec![Out::Reply("Warps: shop".into())]);
        p.command(&player(A, 0), &[body(A, 50.0)], 0, "spawn")
            .unwrap();
        let (_, go) = p.tick(&[body(A, 50.0)], 3_000);
        assert_eq!(
            (go[0].to.x, go[0].done.as_str()),
            (7.0, "Teleported to the spawn.")
        );
        assert_eq!(go[0].spread, 250, "the spawn is fuzzed by default");
        let out = p.command(&player(A, 0), &here, 0, "rules").unwrap();
        assert_eq!(out.len(), 4);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn only_the_spawn_is_spread_over_a_safe_spot() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        let here = [body(A, 7.0)];
        p.command(&player(A, 3), &here, 0, "setspawn").unwrap();
        p.command(&player(A, 3), &here, 0, "setwarp shop").unwrap();
        p.command(&player(A, 3), &here, 0, "sethome").unwrap();
        let far = [body(A, 50.0)];
        p.command(&player(A, 0), &far, 0, "spawn").unwrap();
        let (_, go) = p.tick(&far, 3_000);
        assert_eq!(go[0].to, place(7.0));
        assert_eq!(
            go[0].spread_line().as_deref(),
            Some("execute in minecraft:overworld run spreadplayers 7.000 0.000 0 250 false Alex")
        );
        for (i, line) in ["warp shop", "home"].into_iter().enumerate() {
            let t = 100_000 * (i as i64 + 1);
            p.command(&player(A, 0), &far, t, line).unwrap();
            let (_, go) = p.tick(&far, t + 3_000);
            assert_eq!(go.len(), 1, "{line}");
            assert_eq!(go[0].spread_line(), None, "{line} is exact");
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_zero_radius_sends_players_to_the_exact_spawn() {
        let d = dir();
        let config = Config {
            spawn_radius: 0,
            ..Config::default()
        };
        let mut p = Places::open(&d, config).unwrap();
        let here = [body(A, 7.0)];
        p.command(&player(A, 3), &here, 0, "setspawn").unwrap();
        p.command(&player(A, 0), &here, 0, "spawn").unwrap();
        let (_, go) = p.tick(&here, 3_000);
        assert_eq!(go[0].spread_line(), None);
        let _ = std::fs::remove_dir_all(d);
    }

    /// A chunk of the same column everywhere: `ground`, then `feet` and
    /// `head`, the ground's top at y 64.
    fn chunk(c: (i32, i32), ground: &str, feet: &str, head: &str) -> Ground {
        let palette = vec![ground.to_owned(), feet.to_owned(), head.to_owned()];
        let mut columns = Vec::new();
        for dx in 0..16 {
            for dz in 0..16 {
                columns.push((c.0 * 16 + dx, c.1 * 16 + dz, 65, [0, 1, 2]));
            }
        }
        Ground { palette, columns }
    }
    fn grass(c: (i32, i32)) -> Option<Ground> {
        Some(chunk(
            c,
            "minecraft:grass_block",
            "minecraft:air",
            "minecraft:short_grass",
        ))
    }
    fn distance(t: &Teleport) -> f64 {
        t.to.x.hypot(t.to.z)
    }
    /// An `/rtp` from `here` through its warmup to the teleport, every
    /// chunk read as `land`; the time it ended.
    fn rtp_to(
        p: &mut Places,
        here: &[Body],
        now: i64,
        land: &mut dyn FnMut(u128, (i32, i32)) -> Option<Ground>,
    ) -> (Vec<Out>, Vec<Teleport>) {
        let out = p.command(&player(A, 0), here, now, "rtp").unwrap();
        assert_eq!(
            out,
            vec![Out::Reply("Teleporting in 3 s: don't move.".into())]
        );
        for _ in 0..8 {
            p.rtp_look(land);
        }
        p.tick(here, now + 3_000)
    }

    #[test]
    fn rtp_points_cover_the_ring_evenly() {
        let mut rng = 7;
        let n = 20_000;
        let mut inner = 0;
        for _ in 0..n {
            let (x, z) = ring_point(
                (100.0, -50.0),
                500.0,
                5000.0,
                unit(&mut rng),
                unit(&mut rng),
            );
            let d = (x - 100.0).hypot(z + 50.0);
            assert!((500.0 - 1e-6..=5000.0 + 1e-6).contains(&d), "{d}");
            // Half the ring's area is within sqrt((500² + 5000²) / 2) blocks.
            inner += usize::from(d < ((500f64.powi(2) + 5000f64.powi(2)) / 2.0).sqrt());
        }
        let share = inner as f64 / n as f64;
        assert!(
            (0.48..0.52).contains(&share),
            "uniform over the area, not the radius: {share}"
        );
        // The ring's edges themselves.
        assert_eq!(
            ring_point((0.0, 0.0), 500.0, 5000.0, 0.0, 0.0),
            (500.0, 0.0)
        );
        assert_eq!(
            ring_point((0.0, 0.0), 500.0, 5000.0, 1.0, 0.0),
            (5000.0, 0.0)
        );
    }

    #[test]
    fn rtp_lands_in_the_ring_inside_the_border() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        p.rng = 42;
        p.rng = 1;
        // A border 2,000 blocks wide round the spawn: every chunk tried and
        // every spot is inside it, and 500 blocks out at least.
        p.border = Some([-1000.0, -1000.0, 1000.0, 1000.0]);
        for i in 0..50 {
            let now = i * 10_000_000;
            let here = [body(A, 0.0)];
            let mut asked = Vec::new();
            let (_, go) = rtp_to(&mut p, &here, now, &mut |_, c| {
                asked.push(c);
                grass(c)
            });
            assert_eq!(go.len(), 1);
            let t = &go[0];
            assert!((500.0..=10_000.0).contains(&distance(t)), "{:?}", t.to);
            assert!(t.to.x.abs() <= 984.0 && t.to.z.abs() <= 984.0, "{:?}", t.to);
            assert_eq!((t.to.dimension.as_str(), t.to.y), (OVERWORLD, 65.0));
            assert_eq!(t.spread_line(), None, "the spot is exact");
            assert!(
                t.done.starts_with("Teleported to a random spot "),
                "{}",
                t.done
            );
            for (x, z) in asked {
                assert!((x * 16).abs() <= 1000 && (z * 16).abs() <= 1000, "{x} {z}");
            }
        }
        // A border closer than the ring: nothing to try.
        p.border = Some([-400.0, -400.0, 400.0, 400.0]);
        let out = p
            .command(&player(A, 0), &[body(A, 0.0)], 1_000_000_000, "rtp")
            .unwrap();
        assert!(
            matches!(&out[0], Out::Error(s) if s.contains("world border")),
            "{out:?}"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn rtp_has_its_own_cooldown_kept_over_a_restart() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        p.rng = 42;
        let here = [body(A, 0.0)];
        let (_, go) = rtp_to(&mut p, &here, 0, &mut |_, c| grass(c));
        assert_eq!(go.len(), 1);
        // Other teleports wait only their own short cooldown.
        p.command(&player(A, 3), &here, 20_000, "sethome").unwrap();
        let out = p.command(&player(A, 0), &here, 20_000, "home").unwrap();
        assert!(
            matches!(&out[0], Out::Reply(s) if s.starts_with("Teleporting")),
            "{out:?}"
        );
        p.tick(&[body(A, 1.0)], 20_500);
        let out = p
            .command(&player(A, 0), &here, 3 * 60_000 + 3_000, "rtp")
            .unwrap();
        assert_eq!(
            out,
            vec![Out::Error("Wait 2 min before your next /rtp.".into())]
        );
        let out = p
            .command(&player(A, 0), &here, 300_000 + 2_500, "wild")
            .unwrap();
        assert_eq!(
            out,
            vec![Out::Error("Wait 1 s before your next /rtp.".into())]
        );
        // Written, and read back after a restart.
        p.save(0).unwrap();
        let mut again = Places::open(&d, Config::default()).unwrap();
        again.rng = 43;
        let out = again.command(&player(A, 0), &here, 60_000, "rtp").unwrap();
        assert_eq!(
            out,
            vec![Out::Error("Wait 5 min before your next /rtp.".into())]
        );
        let out = again.command(&player(A, 0), &here, 303_000, "rtp").unwrap();
        assert!(
            matches!(&out[0], Out::Reply(_)),
            "allowed after it: {out:?}"
        );
        // Someone else isn't held up by it.
        let out = again
            .command(&player(B, 0), &[body(B, 0.0)], 60_000, "rtp")
            .unwrap();
        assert!(matches!(&out[0], Out::Reply(_)), "{out:?}");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn rtp_without_a_spot_or_called_off_costs_no_cooldown() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        p.rng = 42;
        let here = [body(A, 0.0)];
        // Ocean everywhere: every chunk tried, one after another, then it says so.
        let mut asked = Vec::new();
        let (out, go) = rtp_to(&mut p, &here, 0, &mut |_, c| {
            asked.push(c);
            Some(chunk(
                c,
                "minecraft:water",
                "minecraft:air",
                "minecraft:air",
            ))
        });
        assert!(go.is_empty());
        assert_eq!(
            out,
            vec![Out::Tell(
                A,
                "No safe spot found for /rtp: try again.".into()
            )]
        );
        assert_eq!(asked.len(), 6, "rtp_attempts chunks: {asked:?}");
        // Land that never loads: given up after rtp_search_seconds.
        let (out, go) = rtp_to(&mut p, &here, 10_000, &mut |_, _| None);
        assert!(go.is_empty());
        assert_eq!(
            out,
            vec![Out::Tell(A, "Looking for a safe spot: don't move.".into())]
        );
        let (out, _) = p.tick(&here, 10_000 + 3_000 + 30_000);
        assert!(out.is_empty(), "{out:?}");
        let (out, _) = p.tick(&here, 10_000 + 3_000 + 30_001);
        assert_eq!(
            out,
            vec![Out::Tell(
                A,
                "/rtp couldn't load the land in time: try again.".into()
            )]
        );
        // The game couldn't make the teleport: no cooldown either.
        let (_, go) = rtp_to(&mut p, &here, 50_000, &mut |_, c| grass(c));
        p.call_off(&go[0]);
        let (_, go) = rtp_to(&mut p, &here, 60_000, &mut |_, c| grass(c));
        assert_eq!(go.len(), 1, "allowed at once");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn rtp_works_only_in_the_overworld() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        p.rng = 42;
        for level in ["minecraft:the_nether", "minecraft:the_end"] {
            let mut b = body(A, 0.0);
            b.at.dimension = level.into();
            let out = p.command(&player(A, 0), &[b], 0, "rtp").unwrap();
            assert_eq!(
                out,
                vec![Out::Error("/rtp only works in the Overworld.".into())]
            );
        }
        assert!(p.pending.is_empty() && p.data.rtp.is_empty());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn rtp_lands_only_on_safe_ground() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        p.rng = 42;
        let here = [body(A, 0.0)];
        // Each chunk tried has one kind of ground, the last a safe one.
        let bad = [
            ("minecraft:lava", "minecraft:air", "minecraft:air"),
            ("minecraft:cactus", "minecraft:air", "minecraft:air"),
            ("minecraft:magma_block", "minecraft:air", "minecraft:air"),
            ("minecraft:powder_snow", "minecraft:air", "minecraft:air"),
            ("minecraft:stone", "minecraft:fire", "minecraft:air"),
            ("minecraft:stone", "minecraft:air", "minecraft:oak_log"),
        ];
        let mut n = 0;
        let (out, go) = rtp_to(&mut p, &here, 0, &mut |_, c| {
            n += 1;
            Some(match bad.get(n - 1) {
                Some((g, f, h)) => chunk(c, g, f, h),
                None => chunk(c, "minecraft:sand", "minecraft:air", "minecraft:air"),
            })
        });
        assert_eq!(n, 6);
        assert!(
            go.is_empty(),
            "the six chunks were all unsafe: {go:?} {out:?}"
        );
        // One safe column among lava: that one.
        let (_, go) = rtp_to(&mut p, &here, 100_000, &mut |_, c| {
            let mut g = chunk(c, "minecraft:lava", "minecraft:air", "minecraft:air");
            g.palette.push("minecraft:dirt".into());
            g.columns[37].3[0] = 3;
            Some(g)
        });
        let (x, z) = (go[0].to.x - 0.5, go[0].to.z - 0.5);
        assert_eq!(x.rem_euclid(16.0), 2.0);
        assert_eq!(z.rem_euclid(16.0), 5.0);
        let _ = std::fs::remove_dir_all(d);
    }

    /// `force` with a fake game: the tickets held, and the commands run.
    fn force_into(
        p: &mut Places,
        now: i64,
        held: &mut BTreeSet<(i32, i32)>,
        ran: &mut Vec<String>,
    ) {
        ran.extend(p.force(now, &mut |c, hold| {
            if hold {
                held.insert(c)
            } else {
                held.remove(&c)
            }
        }));
    }

    #[test]
    fn rtp_holds_its_chunks_with_a_ticket_and_lets_them_go() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        p.rng = 42;
        let here = [body(A, 0.0)];
        p.command(&player(A, 0), &here, 0, "rtp").unwrap();
        let (mut held, mut ran) = (BTreeSet::new(), Vec::new());
        force_into(&mut p, 0, &mut held, &mut ran);
        // One chunk at a time, with a ticket: never `/forceload add`, which
        // waits on the server thread until the chunk is generated.
        assert_eq!(held.len(), 1, "{held:?}");
        assert!(ran.is_empty(), "no command: {ran:?}");
        let first = *held.first().unwrap();
        // The chunk isn't loaded yet: the same one stays held.
        p.rtp_look(&mut |_, _| None);
        force_into(&mut p, 100, &mut held, &mut ran);
        assert_eq!(held, BTreeSet::from([first]));
        // Ocean: the next one is held, the first let go.
        p.rtp_look(&mut |_, c| {
            Some(chunk(
                c,
                "minecraft:water",
                "minecraft:air",
                "minecraft:air",
            ))
        });
        force_into(&mut p, 200, &mut held, &mut ran);
        assert_eq!(held.len(), 1, "{held:?}");
        assert!(!held.contains(&first), "{held:?}");
        // Tickets aren't saved: after a restart nothing is held or let go.
        p.save(0).unwrap();
        let mut again = Places::open(&d, Config::default()).unwrap();
        let (mut held_again, mut ran_again) = (BTreeSet::new(), Vec::new());
        force_into(&mut again, 0, &mut held_again, &mut ran_again);
        assert!(held_again.is_empty() && ran_again.is_empty());
        // Landed: held 10 s more, then let go.
        p.rtp_look(&mut |_, c| grass(c));
        let (_, go) = p.tick(&here, 3_000);
        assert_eq!(go.len(), 1);
        force_into(&mut p, 12_000, &mut held, &mut ran);
        assert_eq!(held.len(), 1, "{held:?}");
        force_into(&mut p, 13_000, &mut held, &mut ran);
        assert!(held.is_empty(), "{held:?}");
        assert!(ran.is_empty(), "{ran:?}");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn chunks_an_older_smp_forced_are_let_go_once() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        p.data.forced.insert((5, -7));
        p.changed = true;
        p.save(0).unwrap();
        let mut again = Places::open(&d, Config::default()).unwrap();
        let (mut held, mut ran) = (BTreeSet::new(), Vec::new());
        force_into(&mut again, 0, &mut held, &mut ran);
        assert_eq!(
            ran,
            vec!["execute in minecraft:overworld run forceload remove 80 -112".to_owned()]
        );
        assert!(held.is_empty());
        // Saved without them: the next start lets go of nothing.
        again.save(0).unwrap();
        let mut third = Places::open(&d, Config::default()).unwrap();
        ran.clear();
        force_into(&mut third, 0, &mut held, &mut ran);
        assert!(ran.is_empty(), "{ran:?}");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_move_calls_an_rtp_off_while_it_looks() {
        let d = dir();
        let mut p = Places::open(&d, Config::default()).unwrap();
        p.rng = 42;
        let here = [body(A, 0.0)];
        p.command(&player(A, 0), &here, 0, "rtp").unwrap();
        let (out, _) = p.tick(&here, 3_000);
        assert_eq!(
            out,
            vec![Out::Tell(A, "Looking for a safe spot: don't move.".into())]
        );
        let (out, go) = p.tick(&[body(A, 2.0)], 4_000);
        assert!(go.is_empty());
        assert_eq!(
            out,
            vec![Out::Tell(A, "Teleport called off: you moved.".into())]
        );
        assert!(p.rtp_wanted(4_000).is_empty(), "its chunk is let go");
        let out = p.command(&player(A, 0), &here, 5_000, "rtp").unwrap();
        assert!(matches!(&out[0], Out::Reply(_)), "no cooldown: {out:?}");
        let _ = std::fs::remove_dir_all(d);
    }
}
