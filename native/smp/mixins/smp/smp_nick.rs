// Nicknames: `/nick <name>` shows another name for you in chat and the Tab
// list, always as `~name` so it never passes for a real name (hovering it
// gives your real one); `/nick off` your name again; operators of level 3
// `/nick <player> <name|off>`; anyone `/realname <nick>`. With `ops_only`
// (the default since SMP 0.22) only operators of level 3 pick nicknames,
// for themselves or for a player; a player may still take off the one an
// operator gave them, and the nicknames players picked themselves are taken
// off when the server starts. A nickname is 3 to
// 16 letters, digits or `_`, and once read without case, `_`, digits and
// look-alikes (0 as o, 1 and i as l...) it may not be or contain a name the
// server knows (anyone who played here, an operator, the Team in
// `protected`, another player's nickname) nor a word kept for the server
// (`reserved`: server, admin, notch...); it must pass the server's chat
// filter; one change every 10 minutes, and only after `min_playtime_minutes`
// of play. Nicknames that break these rules are taken off when the server
// starts. `/pay`, `/tpa`, `/trade`... take a nickname as well as a name. The
// real name stays everywhere else (the log, the anticheat, the Discord
// bridge). Kept in `nicks.json`. A change is decided
// here and made by the glue at the next second, outside the player's
// command, where the chat filter and the Tab list are in reach. No game
// types. Added to the game as `game::smp_nick`; also a module of this crate,
// for its tests.

use super::smp_economy::{Out, Player};
use super::smp_store::{parse_uuid, read_json, save_json, uuid_string};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

pub const COMMANDS: [&str; 2] = ["nick", "realname"];

/// `config.json`'s `nicks`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    /// Only operators of level 3 set nicknames (their own, or a player's).
    pub ops_only: bool,
    /// Between two changes of one's own nickname.
    pub cooldown_seconds: i64,
    /// The play a player needs here before their first nickname.
    pub min_playtime_minutes: i64,
    /// Names no nickname may look like, besides the players' and the
    /// operators': the Team's.
    pub protected: Vec<String>,
    /// Words no nickname may look like (contain, from 4 letters on; be, or
    /// have as a `_` part, when shorter).
    pub reserved: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        let list = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect();
        Self {
            enabled: true,
            ops_only: true,
            cooldown_seconds: 600,
            min_playtime_minutes: 60,
            protected: list(&[
                "rdbt",
                "rdbtclient",
                "luzW",
                "bunchy7s",
                "lu4ovic",
                "ludovic",
                "dezer-x",
                "dezerx",
                "levzzz",
            ]),
            reserved: list(&[
                "server",
                "console",
                "admin",
                "mod",
                "moderator",
                "owner",
                "staff",
                "op",
                "horizon",
                "kira",
                "system",
                "error",
                "notch",
                "jeb",
                "dinnerbone",
                "mojang",
                "microsoft",
                "herobrine",
            ]),
        }
    }
}

/// A name as it reads: lowercase, look-alikes folded (`0` o, `1` `i` `|` l,
/// `3` e, `4` `@` a, `5` `$` s, `7` t, `8` b, `9` g), anything else that
/// isn't a letter (`_`, `-`, other digits) dropped.
pub fn skeleton(name: &str) -> String {
    name.chars()
        .filter_map(|c| match c.to_ascii_lowercase() {
            '0' => Some('o'),
            '1' | 'i' | '|' => Some('l'),
            '3' => Some('e'),
            '4' | '@' => Some('a'),
            '5' | '$' => Some('s'),
            '7' => Some('t'),
            '8' => Some('b'),
            '9' => Some('g'),
            c if c.is_ascii_lowercase() => Some(c),
            _ => None,
        })
        .collect()
}

/// Whether a nickname reading as `nick` (skeleton) copies `name`'s: the
/// same, or containing it when it has 4 letters or more.
fn looks_like(nick: &str, name: &str) -> bool {
    !name.is_empty() && (nick == name || (name.len() >= 4 && nick.contains(name)))
}

/// One player's nickname, when they last changed it (ms), and whether an
/// operator gave it to them (it stays with `ops_only`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Nick {
    pub nick: Option<String>,
    pub changed: i64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub given: bool,
}

/// The level that sets nicknames with `ops_only`, and others' at any time.
pub const OP_LEVEL: u8 = 3;

/// `nicks.json`, by UUID.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct File {
    pub nicks: BTreeMap<String, Nick>,
}

/// A change to make: whose nickname, the new one (None: off), who asked
/// (told the outcome), and whether it starts the cooldown.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub uuid: u128,
    pub nick: Option<String>,
    pub by: u128,
    pub own: bool,
}

/// The names a nickname may not copy: real names of anyone who has played
/// here and of the operators.
pub trait Names {
    /// Every real name the server knows, with its player's UUID when known
    /// (an operator who never played has none).
    fn known(&self) -> Vec<(Option<u128>, String)>;
    /// A player who has played here by name (any case): UUID and name.
    fn find(&self, name: &str) -> Option<(u128, String)>;
    /// A player's real name.
    fn name_of(&self, uuid: u128) -> Option<String>;
    /// A player's operator level (`ops.json`; 0 when not an operator).
    fn level(&self, uuid: u128) -> u8;
}

pub struct Nicks {
    pub config: Config,
    file: File,
    path: PathBuf,
    dirty: bool,
    /// Changes waiting for the next second.
    pub pending: Vec<Change>,
}

/// 3 to 16 of `A-Z a-z 0-9 _`.
pub fn valid(nick: &str) -> bool {
    (3..=16).contains(&nick.len()) && nick.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

impl Nicks {
    /// The nicknames kept in `dir` (`world/horizon-smp/`); an error when
    /// `nicks.json` can't be read (nicknames are off then).
    pub fn open(dir: &std::path::Path, config: Config) -> Result<Self, String> {
        let path = dir.join("nicks.json");
        Ok(Self {
            config,
            file: read_json(&path)?,
            path,
            dirty: false,
            pending: Vec::new(),
        })
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.dirty {
            save_json(&self.path, &self.file)?;
            self.dirty = false;
        }
        Ok(())
    }

    /// Every nickname set, by UUID.
    pub fn all(&self) -> Vec<(u128, String)> {
        self.file
            .nicks
            .iter()
            .filter_map(|(k, n)| Some((parse_uuid(k)?, n.nick.clone()?)))
            .collect()
    }

    pub fn nick_of(&self, uuid: u128) -> Option<&str> {
        self.file.nicks.get(&uuid_string(uuid))?.nick.as_deref()
    }

    /// Whose nickname `nick` is (any case, with or without its `~`).
    pub fn owner(&self, nick: &str) -> Option<u128> {
        let nick = nick.strip_prefix('~').unwrap_or(nick);
        self.all()
            .into_iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(nick))
            .map(|(u, _)| u)
    }

    /// Why `nick` can't be the nickname of `uuid` (real name `real`), or
    /// None: its form, then the server's words, then every name the server
    /// knows and the other nicknames. One's own name never counts.
    pub fn refused(&self, uuid: u128, real: &str, nick: &str, names: &dyn Names) -> Option<String> {
        if !valid(nick) {
            return Some("A nickname is 3 to 16 letters, digits or _.".into());
        }
        let s = skeleton(nick);
        if s.len() < 3 {
            return Some("A nickname needs at least 3 letters.".into());
        }
        let parts: Vec<String> = nick.split('_').map(skeleton).collect();
        for word in &self.config.reserved {
            let w = skeleton(word);
            if looks_like(&s, &w) || parts.contains(&w) {
                return Some(format!(
                    "{nick} reads as \"{word}\": that word is kept for the server."
                ));
            }
        }
        // One's own name as a nickname hides nothing.
        if nick.eq_ignore_ascii_case(real) {
            return None;
        }
        let others = names
            .known()
            .into_iter()
            .filter(|(u, _)| *u != Some(uuid))
            .map(|(_, n)| n)
            .chain(self.config.protected.iter().cloned())
            .chain(
                self.all()
                    .into_iter()
                    .filter(|(u, _)| *u != uuid)
                    .map(|(_, n)| n),
            );
        for name in others {
            // One's own name, also as an `ops.json` or `protected` entry, never counts;
            // another player's does, even when it reads like one's own.
            if !name.eq_ignore_ascii_case(real) && looks_like(&s, &skeleton(&name)) {
                return Some(format!("{nick} looks like the name of another player."));
            }
        }
        None
    }

    /// Takes off the nicknames that break today's rules (from an older
    /// version): each one's UUID, nickname and reason, for the log. With
    /// `ops_only`, those a player who isn't an operator picked themselves.
    pub fn purge(&mut self, names: &dyn Names) -> Vec<(u128, String, String)> {
        let mut gone = Vec::new();
        for (uuid, nick) in self.all() {
            let real = names.name_of(uuid).unwrap_or_default();
            let given = self
                .file
                .nicks
                .get(&uuid_string(uuid))
                .is_some_and(|n| n.given);
            let not_op = (self.config.ops_only && names.level(uuid) < OP_LEVEL && !given)
                .then(|| "nicknames are for operators only".to_owned());
            if let Some(why) = not_op.or_else(|| self.refused(uuid, &real, &nick, names)) {
                if let Some(n) = self.file.nicks.get_mut(&uuid_string(uuid)) {
                    n.nick = None;
                }
                self.dirty = true;
                gone.push((uuid, nick, why));
            }
        }
        gone
    }

    /// `line` with a nickname in a player argument of `/pay`, `/tpa`,
    /// `/trade`... replaced by its player's name; None when nothing to
    /// change. `is_name`: whether a word is a real name (it wins);
    /// `name_of`: a player's real name.
    pub fn resolve(
        &self,
        line: &str,
        is_name: &dyn Fn(&str) -> bool,
        name_of: &dyn Fn(u128) -> Option<String>,
    ) -> Option<String> {
        let mut words: Vec<&str> = line.split_whitespace().collect();
        let at = match words.as_slice() {
            ["trade", "accept" | "deny", _] => 2,
            [
                "pay" | "tpa" | "trade" | "playtime" | "tpaccept" | "tpdeny",
                _,
                ..,
            ] => 1,
            ["eco" | "money", "give" | "take" | "set", _, _] => 2,
            _ => return None,
        };
        let word = words[at];
        if word == "*"
            || (at == 1 && ["accept", "deny", "cancel", "money"].contains(&word))
            || is_name(word)
        {
            return None;
        }
        let real = name_of(self.owner(word)?)?;
        words[at] = &real;
        Some(words.join(" "))
    }

    /// Runs `/nick` or `/realname`; None for another command.
    pub fn command(
        &mut self,
        me: &Player,
        names: &dyn Names,
        played_seconds: i64,
        now: i64,
        line: &str,
    ) -> Option<Vec<Out>> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let (&name, args) = words.split_first()?;
        if !COMMANDS.contains(&name) {
            return None;
        }
        if !self.config.enabled {
            return Some(vec![Out::Error("Nicknames are off on this server.".into())]);
        }
        Some(match (name, args) {
            ("nick", []) => {
                let picks = !self.config.ops_only || me.level >= OP_LEVEL;
                vec![Out::Reply(match (self.nick_of(me.uuid), picks) {
                    (Some(n), true) => {
                        format!(
                            "You go by ~{n}. /nick off takes it off; /nick <name> picks another."
                        )
                    }
                    (Some(n), false) => format!("You go by ~{n}. /nick off takes it off."),
                    (None, true) => {
                        "/nick <name> picks a nickname (shown as ~name); /nick off takes it off."
                            .into()
                    }
                    (None, false) => "Only operators set nicknames on this server.".into(),
                })]
            }
            ("realname", [nick]) => match self
                .owner(nick)
                .and_then(|u| Some((u, self.nick_of(u)?.to_owned())))
            {
                Some((u, n)) => {
                    let real = names.name_of(u).unwrap_or_else(|| uuid_string(u));
                    vec![Out::Reply(format!("~{n} is {real}."))]
                }
                None => vec![Out::Error(format!("Nobody goes by {nick}."))],
            },
            ("realname", _) => vec![Out::Error("Usage: /realname <nickname>".into())],
            ("nick", [new])
                if self.config.ops_only
                    && me.level < OP_LEVEL
                    && !new.eq_ignore_ascii_case("off") =>
            {
                vec![Out::Error(
                    "Only operators set nicknames on this server.".into(),
                )]
            }
            ("nick", [new]) => {
                let need = self.config.min_playtime_minutes;
                if me.level < OP_LEVEL
                    && played_seconds < need * 60
                    && !new.eq_ignore_ascii_case("off")
                {
                    vec![Out::Error(format!(
                        "Nicknames come after {need} minutes of play here; you have played {} so far.",
                        played_seconds / 60
                    ))]
                } else {
                    self.ask(me, me.uuid, &me.name, new, names, now, true)
                }
            }
            ("nick", [who, new]) if me.level >= OP_LEVEL => match names.find(who) {
                Some((uuid, real)) => self.ask(me, uuid, &real, new, names, now, false),
                None => vec![Out::Error(format!(
                    "No player named {who} has played here."
                ))],
            },
            ("nick", [_, _]) => vec![Out::Error(
                "Only operators of level 3 may set another player's nickname.".into(),
            )],
            _ => vec![Out::Error("Usage: /nick <name>, /nick off".into())],
        })
    }

    /// A change for `uuid` (real name `real`), checked; queued when right.
    #[allow(clippy::too_many_arguments)]
    fn ask(
        &mut self,
        me: &Player,
        uuid: u128,
        real: &str,
        new: &str,
        names: &dyn Names,
        now: i64,
        own: bool,
    ) -> Vec<Out> {
        let current = self.nick_of(uuid);
        let new = new.strip_prefix('~').unwrap_or(new);
        let off = new.eq_ignore_ascii_case("off") || new == real;
        if own && let Some(n) = self.file.nicks.get(&uuid_string(uuid)) {
            let wait = n.changed + self.config.cooldown_seconds * 1000 - now;
            if wait > 0 && me.level < OP_LEVEL {
                return vec![Out::Error(format!(
                    "You changed your nickname recently: wait {} more minute(s).",
                    (wait + 59_999) / 60_000
                ))];
            }
        }
        if off {
            if current.is_none() {
                return vec![Out::Error("No nickname to take off.".into())];
            }
        } else {
            if let Some(why) = self.refused(uuid, real, new, names) {
                return vec![Out::Error(why)];
            }
            if current.is_some_and(|c| c == new) {
                return vec![Out::Error(format!(
                    "That is already the nickname of {real}."
                ))];
            }
        }
        self.pending.retain(|c| c.uuid != uuid);
        self.pending.push(Change {
            uuid,
            nick: (!off).then(|| new.to_owned()),
            by: me.uuid,
            own,
        });
        Vec::new()
    }

    /// A pending change the chat filter let through: kept. Lines for the
    /// one who asked and the player.
    pub fn apply(&mut self, change: &Change, real: &str, now: i64) -> Vec<Out> {
        let key = uuid_string(change.uuid);
        let entry = self.file.nicks.entry(key.clone()).or_default();
        entry.nick = change.nick.clone();
        // An operator's gift to another player; one's own pick isn't.
        entry.given = change.nick.is_some() && !change.own;
        if change.own {
            entry.changed = now;
        }
        if entry.nick.is_none() && entry.changed == 0 {
            self.file.nicks.remove(&key);
        }
        self.dirty = true;
        let line = match &change.nick {
            Some(n) => format!("{real} now goes by ~{n}. /nick off takes it off."),
            None => format!("{real} goes by their name again."),
        };
        let mut out = vec![Out::Tell(change.by, line.clone())];
        if change.by != change.uuid {
            out.push(Out::Tell(change.uuid, line));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const ALEX: u128 = 1;
    const STEVE: u128 = 2;
    const HOUR: i64 = 3600;

    struct Known(Vec<(u128, &'static str)>);
    impl Names for Known {
        fn known(&self) -> Vec<(Option<u128>, String)> {
            self.0
                .iter()
                .map(|(u, n)| (Some(*u), (*n).to_owned()))
                .chain([(None, "SomeOp".to_owned())])
                .collect()
        }
        fn find(&self, name: &str) -> Option<(u128, String)> {
            self.0
                .iter()
                .find(|(_, n)| n.eq_ignore_ascii_case(name))
                .map(|(u, n)| (*u, (*n).to_owned()))
        }
        fn name_of(&self, uuid: u128) -> Option<String> {
            self.0
                .iter()
                .find(|(u, _)| *u == uuid)
                .map(|(_, n)| (*n).to_owned())
        }
        fn level(&self, uuid: u128) -> u8 {
            if uuid == OP { 4 } else { 0 }
        }
    }

    /// An operator in `ops.json` (for `Known::level`).
    const OP: u128 = 9;

    /// Nicknames for everyone (the rules before `ops_only`).
    fn open_to_all() -> Config {
        Config {
            ops_only: false,
            ..Config::default()
        }
    }

    fn setup(name: &str) -> (Nicks, std::path::PathBuf) {
        setup_with(name, open_to_all())
    }

    fn setup_with(name: &str, config: Config) -> (Nicks, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("smp-nick-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        (Nicks::open(&dir, config).unwrap(), dir)
    }

    fn player(uuid: u128, name: &str, level: u8) -> Player {
        Player {
            uuid,
            name: name.into(),
            level,
        }
    }

    fn known() -> Known {
        Known(vec![(ALEX, "Alex"), (STEVE, "Steve"), (3, "Bob")])
    }

    /// What `/nick <nick>` answers Alex (played an hour): "" when queued.
    fn try_nick(n: &mut Nicks, nick: &str) -> String {
        n.pending.clear();
        match n
            .command(
                &player(ALEX, "Alex", 0),
                &known(),
                HOUR,
                0,
                &format!("nick {nick}"),
            )
            .unwrap()
            .as_slice()
        {
            [] => String::new(),
            [Out::Error(e)] => e.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn names_read_without_case_underscores_digits_and_look_alikes() {
        assert_eq!(skeleton("N0tch"), "notch");
        assert_eq!(skeleton("D1nnerb0ne_"), "dlnnerbone");
        assert_eq!(skeleton("dinnerbone"), "dlnnerbone");
        assert_eq!(skeleton("lu4ovic"), skeleton("LU4OVIC"));
        assert_eq!(skeleton("$3rv3r"), "server");
        assert_eq!(skeleton("rdbt_2"), "rdbt");
    }

    #[test]
    fn the_form_and_the_letters() {
        let (mut n, dir) = setup("form");
        for bad in ["ab", "way_too_long_nickname", "bad-name", "no.dots"] {
            assert_eq!(
                try_nick(&mut n, bad),
                "A nickname is 3 to 16 letters, digits or _.",
                "{bad}"
            );
        }
        assert_eq!(
            try_nick(&mut n, "1_2_6"),
            "A nickname needs at least 3 letters."
        );
        assert_eq!(try_nick(&mut n, "Sunny"), "");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn words_kept_for_the_server_are_refused_in_any_spelling() {
        let (mut n, dir) = setup("reserved");
        for bad in [
            "Server",
            "SERVER_",
            "s3rv3r",
            "Notch",
            "N0tch",
            "Jeb_",
            "jeb",
            "Dinnerbone",
            "D1nnerb0ne",
            "ERROR",
            "Err0r",
            "TheAdmin",
            "Mod",
            "Mod_Steve",
            "Big_Op",
            "Herobrine2",
            "Mojang",
            "xXConsoleXx",
            "Staff",
            "Kira",
        ] {
            assert!(
                try_nick(&mut n, bad).contains("is kept for the server"),
                "{bad}"
            );
        }
        // Short words count only alone or as a `_` part.
        for good in ["Opal", "Modest", "Jebbo", "Hopper"] {
            assert_eq!(try_nick(&mut n, good), "", "{good}");
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn names_of_players_operators_the_team_and_other_nicknames_are_refused() {
        let (mut n, dir) = setup("names");
        for bad in [
            "steve",
            "St3ve",
            "Steve_",
            "TheSteve",
            "SomeOp",
            "S0me0p",
            "rdbt",
            "RDBT_",
            "rdbtclient",
            "luzW",
            "Lu4ovic",
            "ludovic1",
            "Bunchy7s",
            "levzzz",
            "dezerx",
        ] {
            assert_eq!(
                try_nick(&mut n, bad),
                format!("{bad} looks like the name of another player."),
                "{bad}"
            );
        }
        // A name of 3 letters only when it is the whole nickname.
        assert!(try_nick(&mut n, "bob").contains("another player"));
        assert_eq!(try_nick(&mut n, "Bobcat"), "");
        // One's own name is fine.
        assert_eq!(try_nick(&mut n, "Alex_the_Great"), "");
        // Another player's nickname, in any spelling.
        let rocky = Change {
            uuid: STEVE,
            nick: Some("Rocky".into()),
            by: STEVE,
            own: true,
        };
        n.apply(&rocky, "Steve", 0);
        for bad in ["rocky", "R0cky", "Rocky_2", "RockyRoad"] {
            assert!(try_nick(&mut n, bad).contains("another player"), "{bad}");
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_look_alike_of_another_real_name_is_refused_even_when_it_reads_like_ones_own() {
        let (mut n, dir) = setup("lookalike");
        // "AIex" (capital i) and "Rd8t" read like "Alex" and the Team's "rdbt".
        let k = Known(vec![
            (ALEX, "Alex"),
            (4, "AIex"),
            (STEVE, "Steve"),
            (5, "Rd8t"),
            (OP, "SomeOp"),
        ]);
        let mut ask = |uuid, name, level, nick: &str| {
            n.command(
                &player(uuid, name, level),
                &k,
                HOUR,
                0,
                &format!("nick {nick}"),
            )
            .unwrap()
        };
        assert_eq!(
            ask(4, "AIex", 0, "Alex"),
            [Out::Error(
                "Alex looks like the name of another player.".into()
            )]
        );
        assert_eq!(
            ask(5, "Rd8t", 0, "RDBT"),
            [Out::Error(
                "RDBT looks like the name of another player.".into()
            )]
        );
        // One's own name in another case is no disguise, even with a look-alike around.
        assert_eq!(ask(ALEX, "Alex", 0, "~alex"), []);
        // An operator's own name is also listed without a UUID (`ops.json`).
        assert_eq!(ask(OP, "SomeOp", 4, "SomeOp_the_Great"), []);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_nickname_shows_with_a_tilde_and_waits_for_playtime_and_the_cooldown() {
        let (mut n, dir) = setup("set");
        let k = known();
        let alex = player(ALEX, "Alex", 0);
        assert_eq!(
            n.command(&alex, &k, 59 * 60, 0, "nick Sunny").unwrap(),
            [Out::Error(
                "Nicknames come after 60 minutes of play here; you have played 59 so far.".into()
            )]
        );
        // `/nick` alone says how it works; a `~` typed is not part of it.
        assert!(
            matches!(&n.command(&alex, &k, 0, 0, "nick").unwrap()[0], Out::Reply(r) if r.contains("/nick off"))
        );
        assert_eq!(n.command(&alex, &k, HOUR, 0, "nick ~Sunny").unwrap(), []);
        let c = n.pending.remove(0);
        assert_eq!(c.nick.as_deref(), Some("Sunny"));
        assert_eq!(
            n.apply(&c, "Alex", 1000),
            [Out::Tell(
                ALEX,
                "Alex now goes by ~Sunny. /nick off takes it off.".into()
            )]
        );
        assert!(
            matches!(&n.command(&alex, &k, HOUR, 2000, "nick").unwrap()[0], Out::Reply(r) if r.starts_with("You go by ~Sunny."))
        );
        assert_eq!(
            n.command(&alex, &k, HOUR, 60_000, "nick off").unwrap(),
            [Out::Error(
                "You changed your nickname recently: wait 10 more minute(s).".into()
            )]
        );
        assert_eq!(n.command(&alex, &k, HOUR, 601_000, "nick off").unwrap(), []);
        n.save().unwrap();
        let mut again = Nicks::open(&dir, open_to_all()).unwrap();
        assert_eq!(
            again.nick_of(ALEX),
            Some("Sunny"),
            "kept until the change is applied"
        );
        let steve = player(STEVE, "Steve", 0);
        for asked in ["realname Sunny", "realname ~sunny"] {
            assert_eq!(
                again.command(&steve, &k, 0, 0, asked).unwrap(),
                [Out::Reply("~Sunny is Alex.".into())]
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nicknames_that_break_the_rules_are_taken_off_on_load() {
        let (mut n, dir) = setup("purge");
        for (uuid, nick) in [(ALEX, "Notch"), (STEVE, "Rocky"), (3, "rdbt"), (4, "ERROR")] {
            n.file.nicks.insert(
                uuid_string(uuid),
                Nick {
                    nick: Some(nick.into()),
                    changed: 5,
                    given: false,
                },
            );
        }
        let gone = n.purge(&known());
        let nicks: Vec<&str> = gone.iter().map(|g| g.1.as_str()).collect();
        assert_eq!(nicks, ["Notch", "rdbt", "ERROR"]);
        assert!(gone[0].2.contains("kept for the server"));
        assert_eq!(n.all(), [(STEVE, "Rocky".to_owned())]);
        // The cooldown stays: no new nickname at once.
        assert_eq!(n.file.nicks[&uuid_string(ALEX)].changed, 5);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn commands_that_name_a_player_take_a_nickname() {
        let (mut n, dir) = setup("resolve");
        n.apply(
            &Change {
                uuid: STEVE,
                nick: Some("Rocky".into()),
                by: STEVE,
                own: true,
            },
            "Steve",
            0,
        );
        let is_name = |w: &str| ["alex", "steve"].contains(&w.to_ascii_lowercase().as_str());
        let name_of = |u: u128| known().name_of(u);
        let r = |line: &str| n.resolve(line, &is_name, &name_of);
        assert_eq!(r("pay Rocky 50").as_deref(), Some("pay Steve 50"));
        assert_eq!(r("pay ~rocky 50").as_deref(), Some("pay Steve 50"));
        assert_eq!(r("tpa Rocky").as_deref(), Some("tpa Steve"));
        assert_eq!(r("trade Rocky").as_deref(), Some("trade Steve"));
        assert_eq!(
            r("trade accept Rocky").as_deref(),
            Some("trade accept Steve")
        );
        assert_eq!(r("eco give Rocky 5").as_deref(), Some("eco give Steve 5"));
        assert_eq!(r("pay Steve 50"), None, "a real name stays");
        assert_eq!(r("trade accept"), None);
        assert_eq!(r("eco give * 5"), None);
        assert_eq!(r("pay Nobody 5"), None);
        assert_eq!(r("balance"), None);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn operators_set_others_nicknames_without_a_cooldown_or_playtime() {
        let (mut n, dir) = setup("op");
        let k = known();
        let op = player(ALEX, "Alex", 3);
        assert_eq!(
            n.command(&player(STEVE, "Steve", 0), &k, HOUR, 0, "nick Alex Bobby")
                .unwrap(),
            [Out::Error(
                "Only operators of level 3 may set another player's nickname.".into()
            )]
        );
        assert_eq!(n.command(&op, &k, 0, 0, "nick Steve Rocky").unwrap(), []);
        let c = n.pending.remove(0);
        assert_eq!(
            n.apply(&c, "Steve", 5),
            [
                Out::Tell(
                    ALEX,
                    "Steve now goes by ~Rocky. /nick off takes it off.".into()
                ),
                Out::Tell(
                    STEVE,
                    "Steve now goes by ~Rocky. /nick off takes it off.".into()
                )
            ]
        );
        assert_eq!(n.command(&op, &k, 0, 10, "nick Steve off").unwrap(), []);
        assert_eq!(
            n.command(&op, &k, 0, 10, "nick Nobody x").unwrap(),
            [Out::Error("No player named Nobody has played here.".into())]
        );
        assert_eq!(n.command(&op, &k, 0, 10, "balance"), None);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn only_operators_set_nicknames_and_players_own_ones_go_on_load() {
        let (mut n, dir) = setup_with("ops", Config::default());
        let k = known();
        let alex = player(ALEX, "Alex", 0);
        let op = player(OP, "Boss", 4);
        assert_eq!(
            n.command(&alex, &k, 10 * HOUR, 0, "nick Sunny").unwrap(),
            [Out::Error(
                "Only operators set nicknames on this server.".into()
            )]
        );
        assert!(
            matches!(&n.command(&alex, &k, 0, 0, "nick").unwrap()[0], Out::Reply(r) if r == "Only operators set nicknames on this server.")
        );
        // An operator's own, and one an operator gives a player.
        assert_eq!(n.command(&op, &k, 0, 0, "nick Chief").unwrap(), []);
        assert_eq!(n.command(&op, &k, 0, 0, "nick Steve Rocky").unwrap(), []);
        for c in std::mem::take(&mut n.pending) {
            n.apply(&c, "x", 1);
        }
        // A nickname picked before the rule (not given), which goes.
        n.file.nicks.insert(
            uuid_string(ALEX),
            Nick {
                nick: Some("Sunny".into()),
                changed: 5,
                given: false,
            },
        );
        // The player may take off the one given to them.
        let steve = player(STEVE, "Steve", 0);
        assert!(
            matches!(&n.command(&steve, &k, 0, 9, "nick").unwrap()[0], Out::Reply(r) if r == "You go by ~Rocky. /nick off takes it off.")
        );
        n.save().unwrap();
        let mut again = Nicks::open(&dir, Config::default()).unwrap();
        let gone = again.purge(&k);
        assert_eq!(gone.len(), 1);
        assert_eq!((gone[0].0, gone[0].1.as_str()), (ALEX, "Sunny"));
        assert_eq!(gone[0].2, "nicknames are for operators only");
        let mut left = again.all();
        left.sort();
        assert_eq!(
            left,
            [(STEVE, "Rocky".to_owned()), (OP, "Chief".to_owned())]
        );
        assert_eq!(
            again
                .command(&steve, &k, 0, 10_000_000, "nick off")
                .unwrap(),
            []
        );
        fs::remove_dir_all(dir).ok();
    }
}
