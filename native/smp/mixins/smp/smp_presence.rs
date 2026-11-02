// Who is on, and how: a welcome for a player's first join (a line to
// everyone, the welcome and the rules to the newcomer), a reward once a day
// for being on, AFK players (no move, no turn and no command for
// `afk_minutes`, or `/afk`) left out of the sleep count, and a limit on how
// fast one player runs the SMP's commands. Looked at once a second, never
// per tick. No game types. Added to the game as `game::smp_presence`; also
// a module of this crate, for its tests.

use super::smp_economy::{Out, Player};
use super::smp_store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

pub const COMMANDS: [&str; 1] = ["afk"];

/// `config.json`'s `presence` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The line everyone sees when a player joins for the first time
    /// (`{player}` filled in); empty: none.
    pub first_join: String,
    /// What the newcomer reads, before the rules.
    pub welcome: Vec<String>,
    /// Paid once a day (UTC) to each player who is on; 0: none.
    pub daily_reward: i64,
    /// Minutes of play (moving, not AFK) a day before the reward: an alt
    /// that only logs in gets nothing.
    pub reward_after_minutes: i64,
    /// Minutes without a move, a turn or a command before a player is AFK;
    /// 0: only `/afk`.
    pub afk_minutes: i64,
    /// Whether everyone hears that a player is AFK (else only them).
    pub afk_announce: bool,
    /// At most this many SMP commands in `spam_seconds` a player (operators
    /// of level 3 aside); 0: no limit.
    pub spam_commands: usize,
    pub spam_seconds: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            first_join: "{player} joined for the first time: welcome!".into(),
            welcome: vec![
                "Welcome to the server, {player}!".into(),
                "Set a home with /sethome, claim your land with /claim, see the richest with /baltop.".into(),
            ],
            daily_reward: 50,
            reward_after_minutes: 10,
            afk_minutes: 5,
            afk_announce: false,
            spam_commands: 8,
            spam_seconds: 5,
        }
    }
}

/// `presence.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    /// The players who have joined, by UUID.
    known: BTreeSet<String>,
    /// The last day (UTC, `yyyy-mm-dd`) each player got the daily reward.
    rewarded: BTreeMap<String, String>,
}

/// A player on, as the presence sees them: where they are and look.
pub struct Seen {
    pub uuid: u128,
    pub name: String,
    /// x, y, z, yaw, pitch.
    pub pose: [f64; 5],
}

/// What the game does for the presence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Pays the daily reward (logged as `daily_reward`).
    Reward(u128),
    /// Leaves the player out of the sleep count, or counts them again.
    Sleep(u128, bool),
}

pub struct Presence {
    pub config: Config,
    path: PathBuf,
    data: Data,
    dirty: bool,
    /// Each player on: their pose a second ago, and when they last moved,
    /// turned or ran a command (ms).
    on: HashMap<u128, ([f64; 5], i64)>,
    afk: HashSet<u128>,
    /// Each player's last commands (ms), for the limit.
    recent: HashMap<u128, VecDeque<i64>>,
    /// Seconds each player was active today (UTC day), toward the reward;
    /// not kept over a restart.
    played_today: HashMap<u128, (String, i64)>,
    /// The vanished players (the glue sets it): their first join and AFK
    /// are told to them only, never announced.
    pub quiet: HashSet<u128>,
}

impl Presence {
    /// Opens `presence.json`. Without it, the players `known` already (the
    /// economy's accounts) aren't welcomed as newcomers.
    pub fn open(
        dir: &Path,
        config: Config,
        known: impl IntoIterator<Item = u128>,
    ) -> Result<Self, String> {
        let path = dir.join("presence.json");
        let fresh = !path.exists();
        let mut data: Data = smp_store::read_json(&path)?;
        if fresh {
            data.known = known.into_iter().map(smp_store::uuid_string).collect();
        }
        Ok(Self {
            config,
            path,
            data,
            dirty: fresh,
            on: HashMap::new(),
            afk: HashSet::new(),
            recent: HashMap::new(),
            played_today: HashMap::new(),
            quiet: HashSet::new(),
        })
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.dirty {
            smp_store::write_json(&self.path, &self.data)?;
            self.dirty = false;
        }
        Ok(())
    }

    /// Once a second: who joined, who left, who moved; `today` is the UTC
    /// date, `rules` `/rules`' lines.
    pub fn tick(
        &mut self,
        players: &[Seen],
        today: &str,
        now: i64,
        rules: &[String],
    ) -> (Vec<Out>, Vec<Act>) {
        let (mut out, mut acts) = (Vec::new(), Vec::new());
        let gone: Vec<u128> = self
            .on
            .keys()
            .filter(|u| !players.iter().any(|p| p.uuid == **u))
            .copied()
            .collect();
        for uuid in gone {
            self.on.remove(&uuid);
            self.recent.remove(&uuid);
            if self.afk.remove(&uuid) {
                acts.push(Act::Sleep(uuid, false));
            }
        }
        for p in players {
            let key = smp_store::uuid_string(p.uuid);
            let Some((pose, active)) = self.on.get_mut(&p.uuid) else {
                // Joined.
                self.on.insert(p.uuid, (p.pose, now));
                if self.data.known.insert(key.clone()) {
                    self.dirty = true;
                    let fill = |l: &str| l.replace("{player}", &p.name);
                    if !self.config.first_join.is_empty() && !self.quiet.contains(&p.uuid) {
                        out.push(Out::All(fill(&self.config.first_join)));
                    }
                    out.extend(
                        self.config
                            .welcome
                            .iter()
                            .map(|l| Out::Tell(p.uuid, fill(l))),
                    );
                    if !rules.is_empty() {
                        out.push(Out::Tell(p.uuid, "The rules:".into()));
                        out.extend(
                            rules
                                .iter()
                                .enumerate()
                                .map(|(i, r)| Out::Tell(p.uuid, format!("{}. {r}", i + 1))),
                        );
                    }
                }
                continue;
            };
            let moved = pose.iter().zip(&p.pose).enumerate().any(|(i, (a, b))| {
                // A hundredth of a block, half a degree.
                (a - b).abs() > if i < 3 { 0.01 } else { 0.5 }
            });
            *pose = p.pose;
            if moved {
                *active = now;
                if self.afk.remove(&p.uuid) {
                    out.extend(self.back(p.uuid, &p.name, &mut acts));
                }
            } else if self.config.afk_minutes > 0
                && now - *active >= self.config.afk_minutes * 60_000
                && self.afk.insert(p.uuid)
            {
                out.push(self.away(p.uuid, &p.name, &mut acts));
            }
        }
        // Once a player has been active long enough today (moved or ran a
        // command in the last minute, each second counts).
        if self.config.daily_reward > 0 {
            let need = self.config.reward_after_minutes.max(0) * 60;
            for p in players {
                let key = smp_store::uuid_string(p.uuid);
                if self.data.rewarded.get(&key).map(String::as_str) == Some(today) {
                    continue;
                }
                let active = self.on.get(&p.uuid).is_some_and(|(_, t)| now - t < 60_000);
                let day = self
                    .played_today
                    .entry(p.uuid)
                    .or_insert_with(|| (today.to_owned(), 0));
                if day.0 != today {
                    *day = (today.to_owned(), 0);
                }
                if active {
                    day.1 += 1;
                }
                if day.1 >= need {
                    self.data.rewarded.insert(key, today.to_owned());
                    self.dirty = true;
                    acts.push(Act::Reward(p.uuid));
                }
            }
        }
        (out, acts)
    }

    fn away(&mut self, uuid: u128, name: &str, acts: &mut Vec<Act>) -> Out {
        acts.push(Act::Sleep(uuid, true));
        if self.config.afk_announce && !self.quiet.contains(&uuid) {
            Out::All(format!("{name} is AFK."))
        } else {
            Out::Tell(
                uuid,
                "You are AFK: you don't count for sleeping until you move.".into(),
            )
        }
    }

    fn back(&mut self, uuid: u128, name: &str, acts: &mut Vec<Act>) -> Vec<Out> {
        acts.push(Act::Sleep(uuid, false));
        vec![if self.config.afk_announce && !self.quiet.contains(&uuid) {
            Out::All(format!("{name} is back."))
        } else {
            Out::Tell(uuid, "You are back.".into())
        }]
    }

    /// Before each of a player's SMP commands: counts it as activity, and
    /// refuses it past the limit.
    pub fn allow(&mut self, me: &Player, now: i64) -> Result<(Vec<Out>, Vec<Act>), Out> {
        let (limit, window) = (self.config.spam_commands, self.config.spam_seconds * 1000);
        if limit > 0 && me.level < 3 {
            let recent = self.recent.entry(me.uuid).or_default();
            while recent.front().is_some_and(|t| now - t >= window) {
                recent.pop_front();
            }
            if recent.len() >= limit {
                return Err(Out::Error(format!(
                    "Slow down: at most {limit} commands every {} s.",
                    self.config.spam_seconds
                )));
            }
            recent.push_back(now);
        }
        if let Some((_, active)) = self.on.get_mut(&me.uuid) {
            *active = now;
        }
        let mut acts = Vec::new();
        let out = if self.afk.remove(&me.uuid) {
            self.back(me.uuid, &me.name, &mut acts)
        } else {
            Vec::new()
        };
        Ok((out, acts))
    }

    /// `/afk`: AFK until the player moves, turns or runs a command.
    pub fn command(&mut self, me: &Player, line: &str) -> Option<(Vec<Out>, Vec<Act>)> {
        if line.trim() != "afk" {
            return None;
        }
        let mut acts = Vec::new();
        if !self.afk.insert(me.uuid) {
            return Some((vec![Out::Reply("You are already AFK.".into())], acts));
        }
        let out = self.away(me.uuid, &me.name, &mut acts);
        Some((vec![out], acts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u128 = 1;
    const B: u128 = 2;

    fn seen(uuid: u128, x: f64) -> Seen {
        Seen {
            uuid,
            name: if uuid == A { "Alex" } else { "Steve" }.into(),
            pose: [x, 64.0, 0.0, 0.0, 0.0],
        }
    }

    fn me(uuid: u128) -> Player {
        Player {
            uuid,
            name: "Alex".into(),
            level: 0,
        }
    }

    fn open(known: Vec<u128>) -> (Presence, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "smp-presence-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Presence::open(&dir, Config::default(), known).unwrap(), dir)
    }

    #[test]
    fn a_vanished_player_is_never_announced() {
        let (mut p, _dir) = open(vec![]);
        p.config.afk_announce = true;
        p.quiet.insert(A);
        let (out, _) = p.tick(&[seen(A, 0.0)], "2026-10-31", 0, &[]);
        assert!(
            !out.iter().any(|o| matches!(o, Out::All(_))),
            "no first join: {out:?}"
        );
        let afk = p.config.afk_minutes * 60_000;
        let (out, acts) = p.tick(&[seen(A, 0.0)], "2026-10-31", afk, &[]);
        assert!(
            matches!(&out[..], [Out::Tell(A, _)]),
            "AFK told to them only: {out:?}"
        );
        assert!(acts.contains(&Act::Sleep(A, true)));
        let (out, _) = p.tick(&[seen(A, 5.0)], "2026-10-31", afk + 1_000, &[]);
        assert!(
            matches!(&out[..], [Out::Tell(A, _)]),
            "back told to them only: {out:?}"
        );
    }

    #[test]
    fn a_first_join_is_welcomed_once_and_known_players_are_not() {
        let (mut p, dir) = open(vec![B]);
        let rules = vec!["Be kind.".to_owned()];
        let (out, _) = p.tick(&[seen(A, 0.0), seen(B, 0.0)], "2026-10-31", 0, &rules);
        let all: Vec<_> = out.iter().filter(|o| matches!(o, Out::All(_))).collect();
        assert_eq!(all.len(), 1, "{out:?}");
        assert!(matches!(all[0], Out::All(l) if l.starts_with("Alex joined for the first time")));
        assert!(
            out.iter()
                .any(|o| matches!(o, Out::Tell(A, l) if l == "1. Be kind."))
        );
        assert!(!out.iter().any(|o| matches!(o, Out::Tell(B, _))));
        p.save().unwrap();
        // Back after leaving, or after a restart: no welcome.
        p.tick(&[], "2026-10-31", 1_000, &rules);
        let (out, _) = p.tick(&[seen(A, 0.0)], "2026-10-31", 2_000, &rules);
        assert!(out.is_empty(), "{out:?}");
        let mut again = Presence::open(&dir, Config::default(), []).unwrap();
        let (out, _) = again.tick(&[seen(A, 0.0)], "2026-10-31", 0, &rules);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn the_daily_reward_comes_once_a_day() {
        let (mut p, _) = open(vec![]);
        p.config.reward_after_minutes = 0;
        let (_, acts) = p.tick(&[seen(A, 0.0)], "2026-10-31", 0, &[]);
        assert_eq!(acts, vec![Act::Reward(A)]);
        p.tick(&[], "2026-10-31", 1_000, &[]);
        let (_, acts) = p.tick(&[seen(A, 0.0)], "2026-10-31", 2_000, &[]);
        assert!(acts.is_empty(), "{acts:?}");
        // Midnight with the player on.
        let (_, acts) = p.tick(&[seen(A, 0.0)], "2026-11-01", 3_000, &[]);
        assert_eq!(acts, vec![Act::Reward(A)]);
    }

    #[test]
    fn the_daily_reward_needs_ten_minutes_of_play_so_logging_in_alts_get_nothing() {
        let (mut p, _) = open(vec![A, B]);
        let mut x = 0.0;
        let mut rewarded = Vec::new();
        for s in 0..700 {
            x += 1.0;
            // Alex walks; Steve (an alt) stands still after logging in.
            let (_, acts) = p.tick(&[seen(A, x), seen(B, 0.0)], "2026-10-31", s * 1000, &[]);
            rewarded.extend(acts.into_iter().filter_map(|a| match a {
                Act::Reward(u) => Some((u, s)),
                _ => None,
            }));
        }
        // Alex: rewarded on his 600th second of play. Steve's first minute
        // counts, then nothing.
        assert_eq!(rewarded, vec![(A, 599)]);
        // Leaving and coming back the same day: the count goes on.
        let (mut q, _) = open(vec![A]);
        for s in 0..300 {
            q.tick(&[seen(A, s as f64)], "2026-10-31", s * 1000, &[]);
        }
        q.tick(&[], "2026-10-31", 400_000, &[]);
        let mut got = false;
        for s in 0..302 {
            let (_, acts) = q.tick(&[seen(A, s as f64)], "2026-10-31", 500_000 + s * 1000, &[]);
            got |= acts.contains(&Act::Reward(A));
        }
        assert!(got);
    }

    #[test]
    fn a_still_player_goes_afk_and_comes_back_by_moving_or_a_command() {
        let (mut p, _) = open(vec![A]);
        p.config.daily_reward = 0;
        p.tick(&[seen(A, 0.0)], "d", 0, &[]);
        let (_, acts) = p.tick(&[seen(A, 0.0)], "d", 299_000, &[]);
        assert!(acts.is_empty());
        let (out, acts) = p.tick(&[seen(A, 0.0)], "d", 300_000, &[]);
        assert_eq!(acts, vec![Act::Sleep(A, true)]);
        assert!(matches!(&out[..], [Out::Tell(A, _)]));
        let (_, acts) = p.tick(&[seen(A, 0.0)], "d", 301_000, &[]);
        assert!(acts.is_empty(), "once");
        let (_, acts) = p.tick(&[seen(A, 1.0)], "d", 302_000, &[]);
        assert_eq!(acts, vec![Act::Sleep(A, false)]);
        // /afk, then a command.
        let (_, acts) = p.command(&me(A), "afk").unwrap();
        assert_eq!(acts, vec![Act::Sleep(A, true)]);
        let (_, acts) = p.allow(&me(A), 303_000).unwrap();
        assert_eq!(acts, vec![Act::Sleep(A, false)]);
        // Leaving while AFK counts them again.
        p.command(&me(A), "afk");
        let (_, acts) = p.tick(&[], "d", 304_000, &[]);
        assert_eq!(acts, vec![Act::Sleep(A, false)]);
    }

    #[test]
    fn commands_past_the_limit_wait() {
        let (mut p, _) = open(vec![A]);
        for i in 0..8 {
            assert!(p.allow(&me(A), i * 100).is_ok());
        }
        assert!(p.allow(&me(A), 900).is_err());
        assert!(p.allow(&me(A), 5_000).is_ok(), "the first left the window");
        let op = Player { level: 3, ..me(A) };
        assert!(p.allow(&op, 5_001).is_ok());
    }
}
