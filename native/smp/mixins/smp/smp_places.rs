// Places: the server's spawn (`/spawn`, `/setspawn`), each player's homes
// (`/home`, `/sethome`, `/delhome`, `/homes`), warps (`/warp`, `/warps`,
// `/setwarp`, `/delwarp`), teleport requests between players (`/tpa`,
// `/tpaccept`, `/tpdeny`), `/back` (where the player died, or was before
// their last teleport), `/playtime` and `/rules`. A teleport waits `warmup_seconds`,
// and a player who moves or is hurt meanwhile stays where they are; the glue
// moves them with a vanilla `tp` run as the server. The number of homes grows
// with the time a player has played here, counted by the mod. Everything is
// in `places.json`, written atomically. No game types. Added to the game as
// `game::smp_places`; also a module of this crate, for its tests.

use super::smp_economy::{Out, Player};
use super::smp_store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            warmup_seconds: 3,
            request_seconds: 60,
            cooldown_seconds: 10,
            homes: vec![[0, 1], [5, 2], [20, 3], [100, 5]],
            rules: vec![
                "Be respectful to everyone.".into(),
                "No griefing, no stealing, no cheats.".into(),
                "The currency is earned in game only: never bought or sold for real money.".into(),
            ],
            spawn_radius: 250,
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
    /// The cooldown and `/back` before it: `call_off` puts them back.
    undo: (Option<i64>, Option<(Place, bool)>),
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
pub const COMMANDS: [&str; 16] = [
    "spawn", "setspawn", "home", "sethome", "delhome", "homes", "warp", "warps", "setwarp",
    "delwarp", "tpa", "tpaccept", "tpdeny", "rules", "back", "playtime",
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
    dead: std::collections::HashSet<u128>,
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
        })
    }

    /// Writes `places.json` when homes, warps or the spawn changed, or once
    /// a minute when only the time played did.
    pub fn save(&mut self, now: i64) -> Result<(), String> {
        let played = self.played_at.is_some_and(|t| now - t >= 60_000);
        if !self.changed && !played {
            return Ok(());
        }
        smp_store::write_json(&self.path, &self.data)?;
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
        match t.undo.0 {
            Some(at) => self.last.insert(t.uuid, at),
            None => self.last.remove(&t.uuid),
        };
        match &t.undo.1 {
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
            } else {
                let mut spread = 0;
                let (to, what) = match &p.to {
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
                        let undo = (
                            self.last.insert(p.uuid, now),
                            self.back.insert(p.uuid, (p.from.clone(), false)),
                        );
                        go.push(Teleport {
                            uuid: p.uuid,
                            name: me.name.clone(),
                            to,
                            done: format!("Teleported to {what}."),
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
        let out = p.command(&player(A, 0), &here, 0, "sethome two").unwrap();
        assert!(
            matches!(&out[0], Out::Error(s) if s.contains("1 of 1")),
            "{out:?}"
        );
        // Moving a home is always allowed.
        let out = p.command(&player(A, 0), &here, 0, "sethome home").unwrap();
        assert!(matches!(&out[0], Out::Reply(_)));
        p.save(0).unwrap();
        for _ in 0..5 * 3600 {
            p.played(&[A], 0);
        }
        assert_eq!(p.home_limit(A), 2);
        // Time played is written once a minute.
        p.save(59_999).unwrap();
        assert_eq!(
            Places::open(&d, Config::default()).unwrap().home_limit(A),
            1
        );
        p.save(60_000).unwrap();
        let out = p.command(&player(A, 0), &here, 0, "sethome two").unwrap();
        assert!(matches!(&out[0], Out::Reply(_)), "{out:?}");
        let again = Places::open(&d, Config::default()).unwrap();
        assert_eq!(again.home_limit(A), 2);
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
}
