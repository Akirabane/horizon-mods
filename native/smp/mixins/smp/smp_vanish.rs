// `/vanish` for operators: a vanished operator is out of the other players'
// Tab list and world (whatever their game mode), and nobody reads them
// joining or leaving; operators of `LEVEL` or more still see them. The game
// hides them (`native_hide_player`); this keeps who is vanished in
// `vanish.json`, so it lasts over a restart and a relog, and says what the
// game must be told. The vanished operator reads a subtitle the whole time.
// No game types. Added to the game as `game::smp_vanish`; also a module of
// this crate, for its tests.

use super::smp_economy::{Out, Player};
use super::smp_store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Who may vanish, and who still sees a vanished player.
pub const LEVEL: u8 = 3;

pub const COMMANDS: [&str; 1] = ["vanish"];

/// The subtitle a vanished operator reads the whole time.
pub const SUBTITLE: &str = "Vanished: only operators see you";

/// `vanish.json`: the vanished players by UUID, with the name last seen.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    vanished: BTreeMap<String, String>,
}

pub struct Vanish {
    path: PathBuf,
    data: Data,
    changed: bool,
    /// What the game was told this run (hidden or not), by UUID: a player
    /// whose entry differs from `data` is told again.
    told: HashMap<u128, bool>,
}

impl Vanish {
    /// `vanish.json` in `dir`; an error when it exists but can't be read.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let path = dir.join("vanish.json");
        let data = smp_store::read_json(&path)?;
        Ok(Self {
            path,
            data,
            changed: false,
            told: HashMap::new(),
        })
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.changed {
            smp_store::write_json(&self.path, &self.data)?;
            self.changed = false;
        }
        Ok(())
    }

    pub fn is(&self, uuid: u128) -> bool {
        self.data
            .vanished
            .contains_key(&smp_store::uuid_string(uuid))
    }

    /// The vanished players, by UUID.
    pub fn quiet(&self) -> HashSet<u128> {
        self.data
            .vanished
            .keys()
            .filter_map(|k| smp_store::parse_uuid(k))
            .collect()
    }

    /// Whether a player of `level` sees the player `uuid`.
    pub fn sees(&self, level: u8, uuid: u128) -> bool {
        level >= LEVEL || !self.is(uuid)
    }

    /// What the game must be told now: (UUID, hidden) for each vanished
    /// player not yet hidden this run, and each one shown again.
    pub fn changes(&mut self) -> Vec<(u128, bool)> {
        let mut out = Vec::new();
        for key in self.data.vanished.keys() {
            if let Some(u) = smp_store::parse_uuid(key)
                && self.told.get(&u) != Some(&true)
            {
                out.push((u, true));
            }
        }
        for (&u, &hidden) in &self.told {
            if hidden && !self.is(u) {
                out.push((u, false));
            }
        }
        for &(u, hidden) in &out {
            self.told.insert(u, hidden);
        }
        self.told.retain(|_, hidden| *hidden);
        out
    }

    /// `/vanish` (toggles), `/vanish list`: the answer, and the lines for the
    /// other operators on (`ops`). None for another command.
    pub fn command(&mut self, me: &Player, ops: &[u128], line: &str) -> Option<Vec<Out>> {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.first() != Some(&"vanish") {
            return None;
        }
        if me.level < LEVEL {
            return Some(vec![Out::Error("Only operators can vanish.".into())]);
        }
        let key = smp_store::uuid_string(me.uuid);
        match words[1..] {
            [] => {}
            ["list"] => {
                if self.data.vanished.is_empty() {
                    return Some(vec![Out::Reply("Nobody is vanished.".into())]);
                }
                let names: Vec<&str> = self.data.vanished.values().map(String::as_str).collect();
                return Some(vec![Out::Reply(format!("Vanished: {}.", names.join(", ")))]);
            }
            _ => return Some(vec![Out::Error("Usage: /vanish, /vanish list".into())]),
        }
        self.changed = true;
        let (reply, staff) = if self.data.vanished.remove(&key).is_some() {
            (
                "You are visible again.",
                format!("[Staff] {} is visible again.", me.name),
            )
        } else {
            self.data.vanished.insert(key, me.name.clone());
            (
                "You vanished: other players can't see you, in the Tab list or the world; operators still can.",
                format!("[Staff] {} vanished.", me.name),
            )
        };
        let mut out = vec![Out::Reply(reply.into())];
        out.extend(
            ops.iter()
                .filter(|&&u| u != me.uuid)
                .map(|&u| Out::Tell(u, staff.clone())),
        );
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn dir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("smp-vanish-{}-{n}", std::process::id()))
    }
    fn player(uuid: u128, level: u8) -> Player {
        Player {
            uuid,
            name: format!("P{uuid}"),
            level,
        }
    }

    #[test]
    fn an_operator_vanishes_and_comes_back_and_the_game_is_told_once_each() {
        let d = dir();
        let mut v = Vanish::open(&d).unwrap();
        let out = v.command(&player(1, 3), &[1, 2], "vanish").unwrap();
        assert!(
            matches!(&out[0], Out::Reply(s) if s.starts_with("You vanished")),
            "{out:?}"
        );
        assert_eq!(
            out[1],
            Out::Tell(2, "[Staff] P1 vanished.".into()),
            "the other operators are told"
        );
        assert!(v.is(1));
        assert!(
            !v.sees(0, 1) && !v.sees(2, 1),
            "players below level 3 don't see them"
        );
        assert!(v.sees(3, 1) && v.sees(4, 1), "operators do");
        assert!(v.sees(0, 2), "nobody else is hidden");
        assert_eq!(v.changes(), vec![(1, true)]);
        assert!(v.changes().is_empty(), "told once");
        v.command(&player(1, 4), &[], "vanish").unwrap();
        assert!(!v.is(1));
        assert_eq!(v.changes(), vec![(1, false)]);
        assert!(v.changes().is_empty());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_vanish_lasts_over_a_restart_and_the_game_is_told_again() {
        let d = dir();
        let mut v = Vanish::open(&d).unwrap();
        v.command(&player(1, 3), &[], "vanish").unwrap();
        v.changes();
        v.save().unwrap();
        let mut v = Vanish::open(&d).unwrap();
        assert!(v.is(1));
        assert_eq!(
            v.changes(),
            vec![(1, true)],
            "a new run hides them again, on or off"
        );
        let out = v.command(&player(5, 3), &[], "vanish list").unwrap();
        assert_eq!(out, vec![Out::Reply("Vanished: P1.".into())]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn players_below_level_three_cant_vanish() {
        let d = dir();
        let mut v = Vanish::open(&d).unwrap();
        let out = v.command(&player(1, 2), &[], "vanish").unwrap();
        assert!(matches!(&out[0], Out::Error(_)));
        assert!(!v.is(1) && v.changes().is_empty());
        assert!(v.command(&player(1, 3), &[], "balance").is_none());
        let _ = std::fs::remove_dir_all(d);
    }
}
