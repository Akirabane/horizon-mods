// Land claims: a player claims the chunk they stand in (`/claim`), all its
// height, and only they and the players they trust (`/trust`) break blocks
// or right click blocks there (doors, chests, levers, placing, buckets on a
// block), or hit its frames, armor stands and animals. Nothing flows, burns
// or is pushed into it from land of someone else, and explosions spare it.
// The chunks a player may hold grow with the time they played here.
// A chunk that holds an End portal frame can't be claimed, and the blocks around a frame are
// never protected, so a stronghold's portal stays reachable for everyone. Operators of level 3
// pass everywhere and may `/unclaim` any chunk. Kept in
// `claims.json`, written atomically. No game types. Added to the game as
// `game::smp_claims`; also a module of this crate, for its tests.

use super::smp_economy::{Out, Player};
use super::smp_places::Place;
use super::smp_store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// `config.json`'s `claims` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    /// Chunks a player may claim, by hours played here: `[hours, chunks]`,
    /// the last line reached counts.
    pub chunks: Vec<[i64; 2]>,
    /// The entities only the owner and trusted players may hit in a claim.
    pub protected_entities: Vec<String>,
    /// Blocks around 0 0, in every dimension, where no one may claim (free
    /// for all); 0 for none. Claims already there are dropped at start.
    pub free_radius: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            chunks: vec![[0, 4], [5, 16], [20, 36], [100, 64]],
            protected_entities: PROTECTED
                .iter()
                .map(|&e| format!("minecraft:{e}"))
                .collect(),
            free_radius: 0,
        }
    }
}

/// Frames, stands, vehicles and the animals a player keeps: not monsters.
/// Points along a chunk's four edges, every `step` blocks, at height `y`:
/// where `/claim show` draws its border.
pub fn border(cx: i32, cz: i32, y: f64, step: usize) -> Vec<[f64; 3]> {
    let (x0, z0) = (f64::from(cx * 16), f64::from(cz * 16));
    let mut points = Vec::new();
    for i in (0..=16).step_by(step.max(1)) {
        let t = f64::from(i);
        points.push([x0 + t, y, z0]);
        points.push([x0 + t, y, z0 + 16.0]);
        if i != 0 && i != 16 {
            points.push([x0, y, z0 + t]);
            points.push([x0 + 16.0, y, z0 + t]);
        }
    }
    points
}

const PROTECTED: &[&str] = &[
    "item_frame",
    "glow_item_frame",
    "armor_stand",
    "painting",
    "end_crystal",
    "leash_knot",
    "minecart",
    "chest_minecart",
    "furnace_minecart",
    "hopper_minecart",
    "tnt_minecart",
    "oak_boat",
    "spruce_boat",
    "birch_boat",
    "jungle_boat",
    "acacia_boat",
    "dark_oak_boat",
    "mangrove_boat",
    "cherry_boat",
    "pale_oak_boat",
    "bamboo_raft",
    "oak_chest_boat",
    "spruce_chest_boat",
    "birch_chest_boat",
    "jungle_chest_boat",
    "acacia_chest_boat",
    "dark_oak_chest_boat",
    "mangrove_chest_boat",
    "cherry_chest_boat",
    "pale_oak_chest_boat",
    "bamboo_chest_raft",
    "allay",
    "armadillo",
    "axolotl",
    "bee",
    "camel",
    "cat",
    "chicken",
    "cow",
    "donkey",
    "fox",
    "frog",
    "goat",
    "happy_ghast",
    "horse",
    "llama",
    "mooshroom",
    "mule",
    "ocelot",
    "panda",
    "parrot",
    "pig",
    "rabbit",
    "sheep",
    "sniffer",
    "snow_golem",
    "iron_golem",
    "copper_golem",
    "strider",
    "turtle",
    "villager",
    "wolf",
    "trader_llama",
    "wandering_trader",
];

/// `claims.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    /// Claimed chunks, `<dimension> <chunk x> <chunk z>`, to their owner's UUID.
    chunks: BTreeMap<String, String>,
    /// The players each owner trusts, by UUID.
    trust: BTreeMap<String, BTreeSet<String>>,
}

pub struct Claims {
    pub config: Config,
    path: PathBuf,
    data: Data,
    changed: bool,
    /// `data.chunks` by dimension, then (chunk x, chunk z): asked on every
    /// fluid flow, so a lookup allocates nothing.
    owners: HashMap<String, HashMap<(i32, i32), u128>>,
    /// `data.trust` as (owner, trusted) pairs: asked on every flow across a
    /// border between two claims, so a lookup allocates nothing.
    trusted: HashSet<(u128, u128)>,
    /// When each player was last told a chunk isn't theirs.
    told: HashMap<u128, i64>,
}

/// What sent a projectile, for `Claims::impact_allowed`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    /// A player (UUID, operator level).
    Player(u128, u8),
    /// A mob (a skeleton's arrow, a blaze's fireball).
    Mob,
    /// Nobody (a dispenser's arrow): the block it was launched from.
    Launched([i32; 3]),
    /// Nobody, launched from where isn't known.
    Unknown,
}

fn key(dimension: &str, cx: i32, cz: i32) -> String {
    format!("{dimension} {cx} {cz}")
}

fn chunk_of(at: &Place) -> (i32, i32) {
    ((at.x.floor() as i32) >> 4, (at.z.floor() as i32) >> 4)
}

impl Claims {
    /// `claims.json` in `dir`; an error when it exists but can't be read
    /// (never replaced by an empty one).
    pub fn open(dir: &Path, config: Config) -> Result<Self, String> {
        let path = dir.join("claims.json");
        let data: Data = smp_store::read_json(&path)?;
        let mut owners: HashMap<String, HashMap<(i32, i32), u128>> = HashMap::new();
        for (k, owner) in &data.chunks {
            let mut w = k.split(' ');
            let (Some(d), Some(x), Some(z), Some(u)) = (
                w.next(),
                w.next().and_then(|x| x.parse().ok()),
                w.next().and_then(|z| z.parse().ok()),
                smp_store::parse_uuid(owner),
            ) else {
                return Err(format!("{}: a bad claim `{k}`", path.display()));
            };
            owners.entry(d.to_owned()).or_default().insert((x, z), u);
        }
        let trusted = data
            .trust
            .iter()
            .filter_map(|(o, set)| Some((smp_store::parse_uuid(o)?, set)))
            .flat_map(|(o, set)| {
                set.iter()
                    .filter_map(move |t| Some((o, smp_store::parse_uuid(t)?)))
            })
            .collect();
        Ok(Self {
            config,
            path,
            data,
            changed: false,
            owners,
            trusted,
            told: HashMap::new(),
        })
    }

    /// Whether a chunk touches the free zone around 0 0 (any dimension).
    fn in_free_zone(&self, cx: i32, cz: i32) -> bool {
        let r = self.config.free_radius;
        let near = |c: i32| i64::from(c) * 16 <= r && i64::from(c) * 16 + 15 >= -r;
        r > 0 && near(cx) && near(cz)
    }

    /// Drops the claims inside the free zone (a radius set after they were
    /// made): one line each, for the log.
    pub fn clear_free_zone(&mut self) -> Vec<String> {
        let inside: Vec<(String, i32, i32, u128)> = self
            .owners
            .iter()
            .flat_map(|(d, c)| {
                c.iter()
                    .filter(|((x, z), _)| self.in_free_zone(*x, *z))
                    .map(|((x, z), u)| (d.clone(), *x, *z, *u))
            })
            .collect();
        let mut lines = Vec::new();
        for (dimension, cx, cz, owner) in inside {
            if let Some(c) = self.owners.get_mut(&dimension) {
                c.remove(&(cx, cz));
            }
            self.data.chunks.remove(&key(&dimension, cx, cz));
            self.changed = true;
            lines.push(format!(
                "chunk {cx} {cz} (x {}, z {}) in {dimension} of {} is in the free zone: unclaimed",
                cx * 16,
                cz * 16,
                smp_store::uuid_string(owner)
            ));
        }
        lines.sort();
        lines
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.changed {
            smp_store::save_json(&self.path, &self.data)?;
            self.changed = false;
        }
        Ok(())
    }

    /// Who holds the chunk of this block, if anyone.
    pub fn owner(&self, dimension: &str, x: i32, z: i32) -> Option<u128> {
        self.chunk_owner(dimension, x >> 4, z >> 4)
    }

    fn chunk_owner(&self, dimension: &str, cx: i32, cz: i32) -> Option<u128> {
        self.owners.get(dimension)?.get(&(cx, cz)).copied()
    }

    /// Whether the world may change the block at `to` from `from` (a fluid,
    /// fire, a piston): yes, unless `to` is claimed by someone other than
    /// `from`'s owner or a player they trust.
    pub fn spread_allowed(&self, dimension: &str, from: [i32; 3], to: [i32; 3]) -> bool {
        if !self.config.enabled {
            return true;
        }
        let Some(owner) = self.owner(dimension, to[0], to[2]) else {
            return true;
        };
        match self.owner(dimension, from[0], from[2]) {
            Some(source) => source == owner || self.trusts(owner, source),
            None => false,
        }
    }

    /// Takes out of an explosion's blocks those in a claim: explosions
    /// spare claimed land, whoever lit them.
    pub fn spare(&self, dimension: &str, blocks: &mut Vec<[i32; 3]>) {
        if self.config.enabled && self.owners.get(dimension).is_some_and(|c| !c.is_empty()) {
            blocks.retain(|b| self.owner(dimension, b[0], b[2]).is_none());
        }
    }

    /// Whose land a block is to `who`: None when free, true when theirs or
    /// trusted to them, false when someone else's.
    pub fn relation(&self, dimension: &str, x: i32, z: i32, who: u128) -> Option<bool> {
        let owner = self.owner(dimension, x, z)?;
        Some(owner == who || self.trusts(owner, who))
    }

    /// Whether entities of type `kind` (`minecraft:cow`) are kept from
    /// players a claim's owner doesn't trust. End crystals always are, as
    /// worlds whose config.json predates them keep their own list: one hit
    /// sets a crystal off.
    pub fn protects(&self, kind: &str) -> bool {
        kind == "minecraft:end_crystal" || self.config.protected_entities.iter().any(|e| e == kind)
    }

    fn trusts(&self, owner: u128, who: u128) -> bool {
        self.trusted.contains(&(owner, who))
    }

    /// Whether any claim lies within `r` blocks (x and z) of `at`: four
    /// chunk lookups for `r` < 16, so a blast far from every claim costs
    /// nothing more.
    pub fn near(&self, dimension: &str, at: [i32; 3], r: i32) -> bool {
        if !self.config.enabled {
            return false;
        }
        let Some(chunks) = self.owners.get(dimension).filter(|c| !c.is_empty()) else {
            return false;
        };
        let (x0, x1, z0, z1) = (
            (at[0] - r) >> 4,
            (at[0] + r) >> 4,
            (at[2] - r) >> 4,
            (at[2] + r) >> 4,
        );
        (x0..=x1).any(|cx| (z0..=z1).any(|cz| chunks.contains_key(&(cx, cz))))
    }

    /// Whether lava may set fire at `pos`: lava lights blocks up to 3 blocks
    /// away (x and z; `LavaFluid.randomTick`) and the hook doesn't say from
    /// where, so a claimed block within 3 blocks of land of another owner is
    /// kept, and lava deeper inside a claim still lights its owner's blocks.
    pub fn lava_may_ignite(&self, dimension: &str, pos: [i32; 3]) -> bool {
        [[-3, -3], [-3, 3], [3, -3], [3, 3]]
            .iter()
            .all(|[dx, dz]| self.spread_allowed(dimension, [pos[0] + dx, pos[1], pos[2] + dz], pos))
    }

    /// Whether a projectile from `source` may hit what is at `at`: a block
    /// (`entity` None) or an entity of type `entity` (`minecraft:cow`).
    /// Free land, entities claims don't protect (monsters, players), a
    /// member's (or an operator's) shot and a mob's hit on an entity are
    /// allowed; a shot from nobody counts as from the land it was launched
    /// from (a dispenser inside the claim is its owner's).
    pub fn impact_allowed(
        &self,
        dimension: &str,
        at: [i32; 3],
        entity: Option<&str>,
        source: Source,
    ) -> bool {
        if !self.config.enabled {
            return true;
        }
        let Some(owner) = self.owner(dimension, at[0], at[2]) else {
            return true;
        };
        // An entity Horizon doesn't name (a painting, a frame) counts as kept.
        if entity.is_some_and(|k| !k.is_empty() && !self.protects(k)) {
            return true;
        }
        match source {
            Source::Player(u, level) => level >= 3 || u == owner || self.trusts(owner, u),
            Source::Mob => entity.is_some(),
            Source::Launched(from) => self.spread_allowed(dimension, from, at),
            Source::Unknown => false,
        }
    }

    /// Whether `who` may break or use the block at `pos`. When not, the line
    /// to tell them: at most one each 2 s, so holding a click doesn't flood
    /// their chat.
    pub fn check(
        &mut self,
        who: &Player,
        dimension: &str,
        pos: [i32; 3],
        name_of: &dyn Fn(u128) -> String,
        now: i64,
    ) -> Result<(), Option<String>> {
        if !self.config.enabled || who.level >= 3 {
            return Ok(());
        }
        let Some(owner) = self.owner(dimension, pos[0], pos[2]) else {
            return Ok(());
        };
        if owner == who.uuid || self.trusts(owner, who.uuid) {
            return Ok(());
        }
        let last = self.told.insert(who.uuid, now);
        if last.is_some_and(|t| now - t < 2_000) {
            return Err(None);
        }
        Err(Some(format!("This land is claimed by {}.", name_of(owner))))
    }

    /// How many chunks `uuid` holds.
    pub fn held(&self, uuid: u128) -> usize {
        self.owners
            .values()
            .flat_map(|c| c.values())
            .filter(|&&u| u == uuid)
            .count()
    }

    /// The chunks `owner` holds: (dimension, chunk x, chunk z), sorted.
    pub fn chunks_of(&self, owner: u128) -> Vec<(String, i32, i32)> {
        let mut chunks: Vec<(String, i32, i32)> = self
            .owners
            .iter()
            .flat_map(|(d, c)| {
                c.iter()
                    .filter(|(_, u)| **u == owner)
                    .map(|((x, z), _)| (d.clone(), *x, *z))
            })
            .collect();
        chunks.sort();
        chunks
    }

    /// The players `owner` trusts.
    pub fn trusted_by(&self, owner: u128) -> Vec<u128> {
        self.data
            .trust
            .get(&smp_store::uuid_string(owner))
            .into_iter()
            .flatten()
            .filter_map(|u| smp_store::parse_uuid(u))
            .collect()
    }

    /// The chunks within `MAP_RADIUS` of chunk (`cx`, `cz`) as text, north
    /// (lower z) at the top and east (higher x) on the right, one character
    /// per chunk, all of one width in the game's font: `O` the chunk `who`
    /// stands in, `#` theirs, `+` an owner's who trusts them, `X` someone
    /// else's, `-` free.
    pub fn map(&self, dimension: &str, cx: i32, cz: i32, who: u128) -> Vec<String> {
        (cz - MAP_RADIUS..=cz + MAP_RADIUS)
            .map(|z| {
                (cx - MAP_RADIUS..=cx + MAP_RADIUS)
                    .map(|x| match self.chunk_owner(dimension, x, z) {
                        _ if (x, z) == (cx, cz) => 'O',
                        None => '-',
                        Some(u) if u == who => '#',
                        Some(u) if self.trusts(u, who) => '+',
                        Some(_) => 'X',
                    })
                    .collect()
            })
            .collect()
    }

    /// How many chunks a player may hold after `played` seconds here.
    pub fn limit(&self, played: i64) -> usize {
        self.config
            .chunks
            .iter()
            .filter(|[h, _]| played / 3600 >= *h)
            .map(|[_, n]| (*n).max(0) as usize)
            .next_back()
            .unwrap_or(0)
    }

    /// The claims' commands; None for another command. `at` is where the
    /// player stands; `find` a player's UUID and name from a name (on or
    /// known); `name_of` a name from a UUID; `holds_portal` whether a chunk (dimension, chunk
    /// x, chunk z) holds an End portal frame, which can't be claimed.
    #[allow(clippy::too_many_arguments)]
    pub fn command(
        &mut self,
        me: &Player,
        at: &Place,
        played: i64,
        find: &dyn Fn(&str) -> Option<(u128, String)>,
        name_of: &dyn Fn(u128) -> String,
        holds_portal: &dyn Fn(&str, i32, i32) -> bool,
        line: &str,
    ) -> Option<Vec<Out>> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let command = *words.first()?;
        if !COMMANDS.contains(&command) {
            return None;
        }
        if !self.config.enabled {
            return Some(vec![Out::Error("Claims are off on this server.".into())]);
        }
        let (cx, cz) = chunk_of(at);
        let here = at.dimension.as_str();
        let blocks = format!(
            "x {} to {}, z {} to {}",
            cx * 16,
            cx * 16 + 15,
            cz * 16,
            cz * 16 + 15
        );
        let me_key = smp_store::uuid_string(me.uuid);
        Some(match (command, &words[1..]) {
            ("claim", []) => match self.chunk_owner(here, cx, cz) {
                Some(u) if u == me.uuid => vec![Out::Error("This chunk is already yours.".into())],
                Some(u) => vec![Out::Error(format!("This chunk is {}'s.", name_of(u)))],
                None if self.in_free_zone(cx, cz) => vec![Out::Error(format!(
                    "No claims within {} blocks of 0 0: that land is free for all.",
                    self.config.free_radius
                ))],
                None if holds_portal(here, cx, cz) => vec![Out::Error(
                    "An End portal is in this chunk: it stays free so everyone can reach it.".into(),
                )],
                None => {
                    let (held, limit) = (self.held(me.uuid), self.limit(played));
                    if held >= limit {
                        vec![Out::Error(format!(
                            "You hold {held} of {limit} chunks: play longer for more, or /unclaim one."
                        ))]
                    } else {
                        self.data.chunks.insert(key(here, cx, cz), me_key);
                        let chunks = self.owners.entry(here.to_owned()).or_default();
                        chunks.insert((cx, cz), me.uuid);
                        self.changed = true;
                        vec![Out::Reply(format!(
                            "Claimed this chunk ({blocks}), {} of {limit}. /trust <player> lets a friend build.",
                            held + 1
                        ))]
                    }
                }
            },
            ("claim", ["info"]) => vec![Out::Reply(match self.chunk_owner(here, cx, cz) {
                Some(u) => format!("This chunk ({blocks}) is {}'s.", name_of(u)),
                None => format!("This chunk ({blocks}) is free: /claim it."),
            })],
            ("unclaim", []) => match self.chunk_owner(here, cx, cz) {
                Some(u) if u == me.uuid || me.level >= 3 => {
                    if let Some(chunks) = self.owners.get_mut(here) {
                        chunks.remove(&(cx, cz));
                    }
                    self.data.chunks.remove(&key(here, cx, cz));
                    self.changed = true;
                    vec![Out::Reply(format!("Unclaimed this chunk ({blocks})."))]
                }
                Some(u) => vec![Out::Error(format!("This chunk is {}'s.", name_of(u)))],
                None => vec![Out::Error("This chunk isn't claimed.".into())],
            },
            ("claims", ["map"]) => {
                let mut out = vec![Out::Reply(format!(
                    "Claims around chunk {cx} {cz}, north up (O you, # yours, + trusts you, X someone else's, - free):"
                ))];
                out.extend(self.map(here, cx, cz, me.uuid).into_iter().map(Out::Reply));
                out
            }
            ("claims", []) => {
                let mine: Vec<String> = self
                    .data
                    .chunks
                    .iter()
                    .filter(|(_, o)| **o == me_key)
                    .map(|(k, _)| {
                        let w: Vec<&str> = k.split(' ').collect();
                        let n = |s: &str| s.parse::<i32>().unwrap_or(0) * 16;
                        format!(
                            "{} at x {} z {}",
                            w[0].trim_start_matches("minecraft:"),
                            n(w[1]),
                            n(w[2])
                        )
                    })
                    .collect();
                let trusted: Vec<String> = self
                    .data
                    .trust
                    .get(&me_key)
                    .into_iter()
                    .flatten()
                    .filter_map(|u| smp_store::parse_uuid(u))
                    .map(name_of)
                    .collect();
                vec![
                    Out::Reply(format!(
                        "Your chunks ({} of {}): {}",
                        mine.len(),
                        self.limit(played),
                        if mine.is_empty() {
                            "none, /claim where you stand".into()
                        } else {
                            mine.join("; ")
                        }
                    )),
                    Out::Reply(format!(
                        "Trusted: {}",
                        if trusted.is_empty() {
                            "nobody".into()
                        } else {
                            trusted.join(", ")
                        }
                    )),
                ]
            }
            ("trust", [name]) | ("untrust", [name]) => match find(name) {
                None => vec![Out::Error(format!(
                    "No player named {name} has played here."
                ))],
                Some((u, _)) if u == me.uuid => vec![Out::Error("That's you.".into())],
                Some((u, name)) => {
                    let set = self.data.trust.entry(me_key.clone()).or_default();
                    let changed = if command == "trust" {
                        self.trusted.insert((me.uuid, u));
                        set.insert(smp_store::uuid_string(u))
                    } else {
                        self.trusted.remove(&(me.uuid, u));
                        set.remove(&smp_store::uuid_string(u))
                    };
                    if set.is_empty() {
                        self.data.trust.remove(&me_key);
                    }
                    self.changed |= changed;
                    vec![Out::Reply(match (command, changed) {
                        ("trust", true) => {
                            format!("{name} may now build and open things on your land.")
                        }
                        ("trust", false) => format!("You already trust {name}."),
                        (_, true) => format!("{name} no longer may build on your land."),
                        _ => format!("You didn't trust {name}."),
                    })]
                }
            },
            _ => vec![Out::Error(
                "Usage: /claim, /claim info, /claim show, /unclaim, /claims, /claims map, /trust <player>, /untrust <player>"
                    .into(),
            )],
        })
    }
}

/// How many chunks `/claims map` shows on each side of the player's.
const MAP_RADIUS: i32 = 5;

/// How far from an End portal frame, in blocks along each axis, nothing is protected by a claim.
pub const PORTAL_RADIUS: i32 = 4;

/// The blocks within [`PORTAL_RADIUS`] of `pos` along every axis, `pos` included: `pos` is in the
/// room of an End portal frame when one of them is a frame.
pub fn portal_room(pos: [i32; 3]) -> impl Iterator<Item = [i32; 3]> {
    let near = move |c: i32| c - PORTAL_RADIUS..=c + PORTAL_RADIUS;
    near(pos[0])
        .flat_map(move |x| near(pos[1]).flat_map(move |y| near(pos[2]).map(move |z| [x, y, z])))
}

/// The claims' commands.
pub const COMMANDS: [&str; 5] = ["claim", "unclaim", "claims", "trust", "untrust"];

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const A: u128 = 1;
    const B: u128 = 2;

    fn dir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("smp-claims-{}-{n}", std::process::id()))
    }
    fn at(x: f64, z: f64) -> Place {
        Place {
            dimension: "minecraft:overworld".into(),
            x,
            y: 64.0,
            z,
        }
    }
    fn player(uuid: u128, level: u8) -> Player {
        Player {
            uuid,
            name: if uuid == A { "Alex" } else { "Steve" }.into(),
            level,
        }
    }
    /// No chunk holds an End portal frame.
    fn no_portal(_: &str, _: i32, _: i32) -> bool {
        false
    }

    fn find(n: &str) -> Option<(u128, String)> {
        match n.to_ascii_lowercase().as_str() {
            "alex" => Some((A, "Alex".into())),
            "steve" => Some((B, "Steve".into())),
            _ => None,
        }
    }
    fn name_of(u: u128) -> String {
        if u == A { "Alex" } else { "Steve" }.into()
    }

    #[test]
    fn a_border_runs_along_the_chunks_edges() {
        let points = border(1, -1, 70.0, 2);
        // 9 points on each edge, the corners once.
        assert_eq!(points.len(), 32);
        assert!(points.contains(&[16.0, 70.0, -16.0]) && points.contains(&[32.0, 70.0, 0.0]));
        assert!(
            points
                .iter()
                .all(|p| p[0] == 16.0 || p[0] == 32.0 || p[2] == -16.0 || p[2] == 0.0)
        );
        let d = dir();
        let mut c = Claims::open(&d, Config::default()).unwrap();
        c.command(
            &player(A, 0),
            &at(8.0, 8.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "claim",
        )
        .unwrap();
        let ow = "minecraft:overworld";
        assert_eq!(c.relation(ow, 3, 3, A), Some(true));
        assert_eq!(c.relation(ow, 3, 3, B), Some(false));
        assert_eq!(c.relation(ow, 20, 3, B), None);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn no_claims_in_the_free_zone_and_old_ones_are_dropped() {
        let d = dir();
        let mut c = Claims::open(&d, Config::default()).unwrap();
        let alex = player(A, 0);
        for (x, z) in [(8.0, 8.0), (-520.0, 8.0), (600.0, 600.0)] {
            c.command(
                &alex,
                &at(x, z),
                1_000_000,
                &find,
                &name_of,
                &no_portal,
                "claim",
            )
            .unwrap();
        }
        c.save().unwrap();
        let config = Config {
            free_radius: 500,
            ..Config::default()
        };
        let mut c = Claims::open(&d, config).unwrap();
        // Chunk 0 0 and chunk -33 0 (x -528 to -513: outside 500) stay apart.
        assert_eq!(c.clear_free_zone().len(), 1);
        let ow = "minecraft:overworld";
        assert_eq!(c.owner(ow, 8, 8), None);
        assert_eq!(c.owner(ow, -520, 8), Some(A));
        assert_eq!(c.owner(ow, 600, 600), Some(A));
        let out = c
            .command(
                &alex,
                &at(490.0, -490.0),
                1_000_000,
                &find,
                &name_of,
                &no_portal,
                "claim",
            )
            .unwrap();
        assert!(matches!(&out[0], Out::Error(e) if e.contains("free for all")));
        // The zone is the same in every dimension.
        let nether = Place {
            dimension: "minecraft:the_nether".into(),
            ..at(8.0, 8.0)
        };
        let out = c
            .command(
                &alex, &nether, 1_000_000, &find, &name_of, &no_portal, "claim",
            )
            .unwrap();
        assert!(matches!(&out[0], Out::Error(e) if e.contains("free for all")));
        // Saved without them.
        c.save().unwrap();
        let again = Claims::open(&d, Config::default()).unwrap();
        assert_eq!(again.owner(ow, 8, 8), None);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_chunk_holding_an_end_portal_frame_cannot_be_claimed() {
        let d = dir();
        let mut c = Claims::open(&d, Config::default()).unwrap();
        let alex = player(A, 0);
        // The frame is in chunk 3 -2 of the Overworld (x 48 to 63, z -32 to -17).
        let portal = |dimension: &str, cx: i32, cz: i32| {
            dimension == "minecraft:overworld" && (cx, cz) == (3, -2)
        };
        let out = c
            .command(
                &alex,
                &at(50.0, -20.0),
                1_000_000,
                &find,
                &name_of,
                &portal,
                "claim",
            )
            .unwrap();
        assert!(matches!(&out[0], Out::Error(e) if e.contains("End portal")));
        assert_eq!(c.held(A), 0);
        assert_eq!(c.owner("minecraft:overworld", 50, -20), None);
        // The chunk beside it, and the same chunk in another dimension, are claimable.
        let nether = Place {
            dimension: "minecraft:the_nether".into(),
            ..at(50.0, -20.0)
        };
        for place in [at(70.0, -20.0), nether] {
            let out = c
                .command(&alex, &place, 1_000_000, &find, &name_of, &portal, "claim")
                .unwrap();
            assert!(matches!(&out[0], Out::Reply(r) if r.starts_with("Claimed this chunk")));
        }
        assert_eq!(c.held(A), 2);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_portal_room_is_the_nine_by_nine_by_nine_blocks_around() {
        let pos = [-10, 30, 7];
        let room: Vec<[i32; 3]> = portal_room(pos).collect();
        // Four blocks each way along every axis: 9 x 9 x 9, each block once.
        assert_eq!(room.len(), 729);
        assert_eq!(room.iter().collect::<HashSet<_>>().len(), 729);
        assert!(room.contains(&pos));
        assert!(room.contains(&[-14, 26, 3]) && room.contains(&[-6, 34, 11]));
        // Five blocks away on any one axis is outside.
        for outside in [
            [-15, 30, 7],
            [-5, 30, 7],
            [-10, 35, 7],
            [-10, 25, 7],
            [-10, 30, 12],
            [-10, 30, 2],
        ] {
            assert!(!room.contains(&outside), "{outside:?}");
        }
    }

    #[test]
    fn shots_lava_and_blasts_from_outside_spare_a_claim() {
        let d = dir();
        let mut c = Claims::open(&d, Config::default()).unwrap();
        // Alex holds chunk 0 0 (x 0 to 15, z 0 to 15).
        c.command(
            &player(A, 0),
            &at(8.0, 8.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "claim",
        )
        .unwrap();
        let ow = "minecraft:overworld";
        let (cow, zombie) = (Some("minecraft:cow"), Some("minecraft:zombie"));
        // Steve's arrow: not on Alex's blocks or cows, yes on a zombie there
        // and on anything outside; Alex's own and an operator's pass.
        let steve = Source::Player(B, 0);
        assert!(!c.impact_allowed(ow, [5, 64, 5], None, steve));
        assert!(!c.impact_allowed(ow, [5, 64, 5], cow, steve));
        assert!(!c.impact_allowed(ow, [5, 64, 5], Some(""), steve));
        assert!(c.impact_allowed(ow, [5, 64, 5], zombie, steve));
        assert!(c.impact_allowed(ow, [16, 64, 5], cow, steve));
        assert!(c.impact_allowed(ow, [5, 64, 5], cow, Source::Player(A, 0)));
        assert!(c.impact_allowed(ow, [5, 64, 5], None, Source::Player(B, 3)));
        // A skeleton's arrow hurts the cow but sticks in no block; a
        // dispenser shoots in from its own land only.
        assert!(c.impact_allowed(ow, [5, 64, 5], cow, Source::Mob));
        assert!(!c.impact_allowed(ow, [5, 64, 5], None, Source::Mob));
        assert!(c.impact_allowed(ow, [5, 64, 5], None, Source::Launched([2, 64, 2])));
        assert!(!c.impact_allowed(ow, [5, 64, 5], None, Source::Launched([20, 64, 2])));
        assert!(!c.impact_allowed(ow, [5, 64, 5], cow, Source::Unknown));
        // Trusted, Steve shoots in.
        c.command(
            &player(A, 0),
            &at(8.0, 8.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "trust steve",
        )
        .unwrap();
        assert!(c.impact_allowed(ow, [5, 64, 5], None, steve));
        c.command(
            &player(A, 0),
            &at(8.0, 8.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "untrust steve",
        )
        .unwrap();
        assert!(!c.impact_allowed(ow, [5, 64, 5], None, steve));
        // A snowball on a crystal in the claim sets it off for members only.
        let crystal = Some("minecraft:end_crystal");
        assert!(!c.impact_allowed(ow, [5, 64, 5], crystal, steve));
        assert!(c.impact_allowed(ow, [5, 64, 5], crystal, Source::Player(A, 0)));
        // Lava lights claimed blocks only 4 or more blocks from the border.
        assert!(!c.lava_may_ignite(ow, [13, 64, 8]));
        assert!(!c.lava_may_ignite(ow, [8, 64, 2]));
        assert!(c.lava_may_ignite(ow, [8, 64, 8]));
        assert!(c.lava_may_ignite(ow, [30, 64, 8]));
        // A blast near the claim is told apart from one far away.
        assert!(c.near(ow, [20, 64, 8], 6));
        assert!(!c.near(ow, [40, 64, 8], 6));
        assert!(!c.near("minecraft:the_nether", [8, 64, 8], 6));
        // The trust index comes back with the file.
        c.command(
            &player(A, 0),
            &at(8.0, 8.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "trust steve",
        )
        .unwrap();
        c.save().unwrap();
        let again = Claims::open(&d, Config::default()).unwrap();
        assert!(again.impact_allowed(ow, [5, 64, 5], None, steve));
        assert!(again.spread_allowed(ow, [5, 64, 5], [6, 64, 5]));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_world_stays_out_of_a_claim() {
        let d = dir();
        let mut c = Claims::open(&d, Config::default()).unwrap();
        c.command(
            &player(A, 0),
            &at(8.0, 8.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "claim",
        )
        .unwrap();
        let ow = "minecraft:overworld";
        // Into Alex's chunk from outside: refused; inside it and out of it: allowed.
        assert!(!c.spread_allowed(ow, [16, 64, 8], [15, 64, 8]));
        assert!(c.spread_allowed(ow, [14, 64, 8], [15, 64, 8]));
        assert!(c.spread_allowed(ow, [15, 64, 8], [16, 64, 8]));
        assert!(c.spread_allowed("minecraft:the_nether", [16, 64, 8], [15, 64, 8]));
        // From Steve's chunk next door: allowed once Alex trusts Steve.
        c.command(
            &player(B, 0),
            &at(24.0, 8.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "claim",
        )
        .unwrap();
        assert!(!c.spread_allowed(ow, [16, 64, 8], [15, 64, 8]));
        c.command(
            &player(A, 0),
            &at(8.0, 8.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "trust steve",
        )
        .unwrap();
        assert!(c.spread_allowed(ow, [16, 64, 8], [15, 64, 8]));
        assert!(!c.spread_allowed(ow, [15, 64, 8], [16, 64, 8]));
        // Explosions spare both claims, not the land around them.
        let mut blocks = vec![[0, 64, 0], [20, 64, 0], [40, 64, 0], [-1, 64, 0]];
        c.spare(ow, &mut blocks);
        assert_eq!(blocks, vec![[40, 64, 0], [-1, 64, 0]]);
        assert!(c.protects("minecraft:item_frame") && c.protects("minecraft:cow"));
        c.config.protected_entities = vec!["minecraft:cow".into()];
        assert!(
            c.protects("minecraft:end_crystal"),
            "a list from before crystals"
        );
        assert!(!c.protects("minecraft:zombie") && !c.protects("minecraft:player"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_claim_keeps_others_out_until_trusted() {
        let d = dir();
        let mut c = Claims::open(&d, Config::default()).unwrap();
        let out = c
            .command(
                &player(A, 0),
                &at(-3.5, 20.0),
                0,
                &find,
                &name_of,
                &no_portal,
                "claim",
            )
            .unwrap();
        assert!(
            matches!(&out[0], Out::Reply(s) if s.contains("x -16 to -1, z 16 to 31")),
            "{out:?}"
        );
        let ow = "minecraft:overworld";
        assert_eq!(
            c.check(&player(A, 0), ow, [-16, 0, 31], &name_of, 0),
            Ok(())
        );
        assert_eq!(
            c.check(&player(B, 0), ow, [-1, 100, 16], &name_of, 0),
            Err(Some("This land is claimed by Alex.".into()))
        );
        // Told once each 2 s; the next chunk and the Nether are free.
        assert_eq!(
            c.check(&player(B, 0), ow, [-1, 100, 16], &name_of, 1_000),
            Err(None)
        );
        assert_eq!(c.check(&player(B, 0), ow, [0, 64, 16], &name_of, 0), Ok(()));
        assert_eq!(
            c.check(
                &player(B, 0),
                "minecraft:the_nether",
                [-1, 64, 16],
                &name_of,
                0
            ),
            Ok(())
        );
        // Operators pass.
        assert_eq!(
            c.check(&player(B, 3), ow, [-1, 64, 16], &name_of, 0),
            Ok(())
        );
        c.command(
            &player(A, 0),
            &at(0.0, 0.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "trust steve",
        )
        .unwrap();
        assert_eq!(
            c.check(&player(B, 0), ow, [-1, 64, 16], &name_of, 9_000),
            Ok(())
        );
        let out = c
            .command(
                &player(B, 0),
                &at(-3.5, 20.0),
                0,
                &find,
                &name_of,
                &no_portal,
                "unclaim",
            )
            .unwrap();
        assert!(
            matches!(&out[0], Out::Error(s) if s.contains("Alex's")),
            "trust isn't ownership"
        );
        c.save().unwrap();
        let mut again = Claims::open(&d, Config::default()).unwrap();
        assert_eq!(again.owner(ow, -1, 16), Some(A));
        again
            .command(
                &player(A, 0),
                &at(0.0, 0.0),
                0,
                &find,
                &name_of,
                &no_portal,
                "untrust Steve",
            )
            .unwrap();
        assert!(
            again
                .check(&player(B, 0), ow, [-1, 64, 16], &name_of, 20_000)
                .is_err()
        );
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn the_map_shows_who_holds_the_chunks_around_you() {
        let d = dir();
        let mut c = Claims::open(&d, Config::default()).unwrap();
        let (alex, steve) = (player(A, 0), player(B, 0));
        // Alex holds chunks 0 0 and 1 0; Steve holds chunk 0 -2.
        for (who, x, z) in [(&alex, 8.0, 8.0), (&alex, 24.0, 8.0), (&steve, 8.0, -24.0)] {
            c.command(who, &at(x, z), 0, &find, &name_of, &no_portal, "claim")
                .unwrap();
        }
        let ask = |c: &mut Claims, who: &Player, x, z| {
            c.command(who, &at(x, z), 0, &find, &name_of, &no_portal, "claims map")
                .unwrap()
        };
        // Standing in chunk 0 0: a header, then 11 rows of 11 chunks, north
        // (chunk z -5) first, west (chunk x -5) on the left.
        let out = ask(&mut c, &alex, 8.0, 8.0);
        let Out::Reply(header) = &out[0] else {
            panic!("{out:?}")
        };
        assert!(header.contains("chunk 0 0"), "{header}");
        let rows: Vec<&str> = out[1..]
            .iter()
            .map(|o| match o {
                Out::Reply(r) => r.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(rows.len(), 11);
        assert!(rows.iter().all(|r| r.len() == 11));
        assert_eq!(rows[5], "-----O#----", "{rows:?}");
        assert_eq!(rows[3], "-----X-----", "{rows:?}");
        assert_eq!(
            rows.iter()
                .flat_map(|r| r.chars())
                .filter(|&c| c != '-')
                .count(),
            3
        );
        // Steve trusting Alex turns his chunk into a `+` on Alex's map.
        c.command(
            &steve,
            &at(8.0, -24.0),
            0,
            &find,
            &name_of,
            &no_portal,
            "trust alex",
        )
        .unwrap();
        let out = ask(&mut c, &alex, 8.0, 8.0);
        assert!(
            matches!(&out[4], Out::Reply(r) if r == "-----+-----"),
            "{out:?}"
        );
        // From the chunk next door the same claims shift one column left, and
        // Steve sees Alex's chunks as someone else's.
        let out = ask(&mut c, &steve, 24.0, 8.0);
        assert!(
            matches!(&out[6], Out::Reply(r) if r == "----XO-----"),
            "{out:?}"
        );
        assert!(
            matches!(&out[4], Out::Reply(r) if r == "----#------"),
            "{out:?}"
        );
        // The Nether holds none of them; the map is all free but its center.
        let nether = Place {
            dimension: "minecraft:the_nether".into(),
            ..at(8.0, 8.0)
        };
        let out = c
            .command(&alex, &nether, 0, &find, &name_of, &no_portal, "claims map")
            .unwrap();
        assert!(
            matches!(&out[6], Out::Reply(r) if r == "-----O-----"),
            "{out:?}"
        );
        assert!(
            matches!(&out[1], Out::Reply(r) if r == "-----------"),
            "{out:?}"
        );
        // Far from every claim the map is empty, and with claims off it says so.
        let out = ask(&mut c, &alex, 8000.0, 8000.0);
        assert!(
            out[1..]
                .iter()
                .all(|o| matches!(o, Out::Reply(r) if r.chars().all(|ch| ch == '-' || ch == 'O')))
        );
        c.config.enabled = false;
        let out = ask(&mut c, &alex, 8.0, 8.0);
        assert!(
            matches!(&out[0], Out::Error(e) if e.contains("off")),
            "{out:?}"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_map_reaches_five_chunks_each_way_in_negative_coordinates_too() {
        let d = dir();
        let mut c = Claims::open(&d, Config::default()).unwrap();
        let (alex, steve) = (player(A, 0), player(B, 0));
        // The middle of chunk `n` is block `n * 16 + 8`. Alex holds chunk
        // -1 0 (x -16 to -1, west of the origin); Steve the corners 5 chunks
        // away (-5 -5 and 5 5) and a chunk 6 away on each side of 0 0.
        let played = 1_000_000;
        c.command(
            &alex,
            &at(-8.0, 8.0),
            played,
            &find,
            &name_of,
            &no_portal,
            "claim",
        )
        .unwrap();
        for (cx, cz) in [(-5, -5), (5, 5), (-6, 0), (6, 0), (0, -6), (0, 6)] {
            let place = at(f64::from(cx * 16 + 8), f64::from(cz * 16 + 8));
            let out = c
                .command(&steve, &place, played, &find, &name_of, &no_portal, "claim")
                .unwrap();
            assert!(matches!(&out[0], Out::Reply(_)), "{out:?}");
        }
        let rows = |c: &mut Claims, x, z| -> Vec<String> {
            let out = c
                .command(
                    &alex,
                    &at(x, z),
                    played,
                    &find,
                    &name_of,
                    &no_portal,
                    "claims map",
                )
                .unwrap();
            out[1..]
                .iter()
                .map(|o| match o {
                    Out::Reply(r) => r.clone(),
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        // From chunk 0 0: the corners at 5 chunks are on the grid's edge, the
        // chunks 6 away are off it, and Alex's chunk at x -16 to -1 is left of
        // the middle.
        assert_eq!(
            rows(&mut c, 8.0, 8.0),
            [
                "X----------",
                "-----------",
                "-----------",
                "-----------",
                "-----------",
                "----#O-----",
                "-----------",
                "-----------",
                "-----------",
                "-----------",
                "----------X",
            ]
        );
        // From chunk -1 -1, with a negative x and z: Steve's -5 -5 is one
        // column in, his -6 0 on the left edge and his 0 -6 on the top row.
        assert_eq!(
            rows(&mut c, -8.0, -8.0),
            [
                "------X----",
                "-X---------",
                "-----------",
                "-----------",
                "-----------",
                "-----O-----",
                "X----#-----",
                "-----------",
                "-----------",
                "-----------",
                "-----------",
            ]
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn chunks_grow_with_time_played() {
        let d = dir();
        let mut c = Claims::open(&d, Config::default()).unwrap();
        for x in 0..4 {
            let out = c
                .command(
                    &player(A, 0),
                    &at(x as f64 * 16.0, 0.0),
                    0,
                    &find,
                    &name_of,
                    &no_portal,
                    "claim",
                )
                .unwrap();
            assert!(matches!(&out[0], Out::Reply(_)), "{out:?}");
        }
        let out = c
            .command(
                &player(A, 0),
                &at(64.0, 0.0),
                0,
                &find,
                &name_of,
                &no_portal,
                "claim",
            )
            .unwrap();
        assert!(
            matches!(&out[0], Out::Error(s) if s.contains("4 of 4")),
            "{out:?}"
        );
        let out = c
            .command(
                &player(A, 0),
                &at(64.0, 0.0),
                5 * 3600,
                &find,
                &name_of,
                &no_portal,
                "claim",
            )
            .unwrap();
        assert!(
            matches!(&out[0], Out::Reply(s) if s.contains("5 of 16")),
            "{out:?}"
        );
        let out = c
            .command(
                &player(B, 0),
                &at(0.0, 0.0),
                0,
                &find,
                &name_of,
                &no_portal,
                "claim",
            )
            .unwrap();
        assert_eq!(out, vec![Out::Error("This chunk is Alex's.".into())]);
        // An operator may free any chunk.
        let out = c
            .command(
                &player(B, 3),
                &at(0.0, 0.0),
                0,
                &find,
                &name_of,
                &no_portal,
                "unclaim",
            )
            .unwrap();
        assert!(matches!(&out[0], Out::Reply(_)));
        let out = c
            .command(
                &player(A, 0),
                &at(0.0, 0.0),
                0,
                &find,
                &name_of,
                &no_portal,
                "claims",
            )
            .unwrap();
        assert!(
            matches!(&out[0], Out::Reply(s) if s.starts_with("Your chunks (4 of 4)")),
            "{out:?}"
        );
        let _ = std::fs::remove_dir_all(d);
    }
}
