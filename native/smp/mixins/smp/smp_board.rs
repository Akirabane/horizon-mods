// The SMP's screens: a scoreboard sidebar, the player's own block first (their
// name or nickname, balance, playtime, the players on), then the three
// richest and the server's address, which a player hides with `/sidebar off`
// (kept in `board.json`); and the Tab list's header and footer (the server's
// name, its TPS and the players on). Nothing happens per tick: the ranking is
// built once for everyone when a balance changed (at most once each
// `min_seconds`) or every `refresh_seconds`; the players' own lines are looked
// at on the same `refresh_seconds`, and a sidebar is sent only to the players
// whose lines changed and to newcomers. The Tab list goes out every
// `tab_refresh_seconds`, and to a player who just joined. No game types. Added to the game as `game::smp_board`;
// also a module of this crate, for its tests.

use super::smp_economy::{Out, Player};
use super::smp_store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The most of the richest a sidebar lists: vanilla's 15 lines, less the
/// player's four, the two gaps, the heading and the address.
const MAX_TOP: usize = 7;

/// `config.json`'s `board` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The sidebar (each player may hide it).
    pub sidebar: bool,
    pub title: String,
    /// How many of the richest players it lists (at most 7; 0: none).
    pub top: usize,
    /// Its last line, the server's address ("": none).
    pub footer: String,
    /// The ranking is built again, and the players' own lines looked at,
    /// at least this often.
    pub refresh_seconds: i64,
    /// After a balance changed, at most this often.
    pub min_seconds: i64,
    /// The Tab list's header and footer.
    pub tab: bool,
    /// `{server}`, `{tps}`, `{mspt}`, `{players}` are filled in.
    pub tab_header: String,
    pub tab_footer: String,
    pub server_name: String,
    pub tab_refresh_seconds: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sidebar: true,
            title: "Horizon SMP".into(),
            top: 3,
            footer: "play.playhorizon.dev".into(),
            refresh_seconds: 60,
            min_seconds: 5,
            tab: true,
            tab_header: "{server}".into(),
            tab_footer: "TPS {tps} · {players} online".into(),
            server_name: "Horizon SMP".into(),
            tab_refresh_seconds: 5,
        }
    }
}

/// SMP 0.13's sidebar defaults (the richest ten), moved to 0.14's when a
/// server never changed them. True when something changed.
pub fn migrate(config: &mut Config) -> bool {
    let mut changed = false;
    if config.title == "Richest players" {
        config.title = Config::default().title;
        changed = true;
    }
    if config.top == 10 {
        config.top = Config::default().top;
        changed = true;
    }
    changed
}

/// `board.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    /// The players who hid the sidebar, by UUID.
    hidden: BTreeSet<String>,
}

/// A line of a player's sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// The player's name, or their nickname.
    Name(String),
    /// A value of theirs or the server's: `Balance`, `Playtime`, `Online`.
    Stat { label: &'static str, value: String },
    /// An empty line.
    Gap,
    /// The heading over the richest.
    Heading,
    /// One of the richest: their rank, name and balance; `me` on the
    /// player's own line.
    Top {
        rank: usize,
        name: String,
        amount: String,
        me: bool,
    },
    /// The server's address.
    Footer(String),
}

/// What a player's own lines show, from the other modules.
pub struct Me {
    /// Their nickname, else their name.
    pub name: String,
    pub played_seconds: i64,
}

/// `2h 05m`, `12m`: the sidebar's playtime.
pub fn short_duration(seconds: i64) -> String {
    let (h, m) = (seconds / 3600, seconds % 3600 / 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

pub struct Board {
    pub config: Config,
    path: PathBuf,
    data: Data,
    changed: bool,
    /// Every account by balance, richest first: UUID, name, balance.
    ranked: Vec<(u128, String, i64)>,
    rank_of: HashMap<u128, usize>,
    /// When the ranking was built, and over which economy revision.
    built_at: Option<i64>,
    built_revision: u64,
    /// What each player on was last sent.
    sent: HashMap<u128, Vec<Line>>,
    /// Built since the last `updates`: every player's lines are looked at.
    stale: bool,
    /// When the players' own lines were last looked at.
    personal_at: Option<i64>,
    tab_at: Option<i64>,
    tab_sent: HashSet<u128>,
}

/// `1,250`, `125.4k`, `3.2M`: short enough for the sidebar's right side.
pub fn compact(n: i64) -> String {
    let a = n.unsigned_abs();
    let sign = if n < 0 { "-" } else { "" };
    if a < 100_000 {
        let digits = a.to_string();
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        return format!("{sign}{out}");
    }
    let (value, unit) = match a {
        100_000..1_000_000 => (a as f64 / 1e3, "k"),
        1_000_000..1_000_000_000 => (a as f64 / 1e6, "M"),
        _ => (a as f64 / 1e9, "B"),
    };
    format!("{sign}{value:.1}{unit}")
}

impl Board {
    /// `board.json` in `dir`; an error when it exists but can't be read.
    pub fn open(dir: &Path, config: Config) -> Result<Self, String> {
        let path = dir.join("board.json");
        let data = smp_store::read_json(&path)?;
        Ok(Self {
            config,
            path,
            data,
            changed: false,
            ranked: Vec::new(),
            rank_of: HashMap::new(),
            built_at: None,
            built_revision: 0,
            sent: HashMap::new(),
            stale: false,
            personal_at: None,
            tab_at: None,
            tab_sent: HashSet::new(),
        })
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.changed {
            smp_store::write_json(&self.path, &self.data)?;
            self.changed = false;
        }
        Ok(())
    }

    /// Forgets what each player was sent: everyone gets theirs again at the
    /// next update (the sidebars were taken away).
    pub fn forget(&mut self) {
        self.sent.clear();
        self.tab_sent.clear();
        self.stale = true;
    }

    pub fn hidden(&self, uuid: u128) -> bool {
        self.data.hidden.contains(&smp_store::uuid_string(uuid))
    }

    /// Whether the ranking should be built again: the economy changed
    /// (`revision`) and `min_seconds` passed, or `refresh_seconds` passed.
    pub fn due(&self, now: i64, revision: u64) -> bool {
        if !self.config.sidebar {
            return false;
        }
        let Some(at) = self.built_at else {
            return true;
        };
        let since = now - at;
        (revision != self.built_revision && since >= self.config.min_seconds * 1000)
            || since >= self.config.refresh_seconds * 1000
    }

    /// The ranking from the accounts (UUID, name, balance, any order).
    pub fn rebuild(&mut self, mut accounts: Vec<(u128, String, i64)>, now: i64, revision: u64) {
        accounts.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
        self.rank_of = accounts
            .iter()
            .enumerate()
            .map(|(i, a)| (a.0, i + 1))
            .collect();
        self.ranked = accounts;
        self.built_at = Some(now);
        self.built_revision = revision;
        self.stale = true;
    }

    /// `uuid`'s sidebar: their own block, the richest from the ranking, the
    /// address.
    pub fn lines(&self, uuid: u128, me: &Me, online: usize) -> Vec<Line> {
        let balance = self
            .rank_of
            .get(&uuid)
            .map_or(0, |&rank| self.ranked[rank - 1].2);
        let mut lines = vec![
            Line::Name(me.name.clone()),
            Line::Stat {
                label: "Balance",
                value: compact(balance),
            },
            Line::Stat {
                label: "Playtime",
                value: short_duration(me.played_seconds),
            },
            Line::Stat {
                label: "Online",
                value: online.to_string(),
            },
        ];
        let top = self.config.top.min(MAX_TOP);
        if top > 0 && !self.ranked.is_empty() {
            lines.push(Line::Gap);
            lines.push(Line::Heading);
            lines.extend(self.ranked.iter().take(top).enumerate().map(
                |(i, (u, name, balance))| Line::Top {
                    rank: i + 1,
                    name: name.clone(),
                    amount: compact(*balance),
                    me: *u == uuid,
                },
            ));
        }
        if !self.config.footer.is_empty() {
            lines.push(Line::Gap);
            lines.push(Line::Footer(self.config.footer.clone()));
        }
        lines
    }

    /// The sidebars to send now, to the players `on`: those who joined, and
    /// after a rebuild or each `refresh_seconds` those whose lines changed.
    /// `me` is asked only for the players looked at. Players who hid it get
    /// nothing; players gone are forgotten (they get it again on return).
    pub fn updates(
        &mut self,
        now: i64,
        on: &[u128],
        me: impl Fn(u128) -> Me,
    ) -> Vec<(u128, Vec<Line>)> {
        self.sent.retain(|u, _| on.contains(u));
        self.tab_sent.retain(|u| on.contains(u));
        if !self.config.sidebar || self.built_at.is_none() {
            return Vec::new();
        }
        let personal = self
            .personal_at
            .is_none_or(|t| now - t >= self.config.refresh_seconds * 1000);
        if personal {
            self.personal_at = Some(now);
        }
        let all = std::mem::take(&mut self.stale) || personal;
        let mut out = Vec::new();
        for &uuid in on {
            if self.hidden(uuid) || (!all && self.sent.contains_key(&uuid)) {
                continue;
            }
            let lines = self.lines(uuid, &me(uuid), on.len());
            if self.sent.get(&uuid) != Some(&lines) {
                self.sent.insert(uuid, lines.clone());
                out.push((uuid, lines));
            }
        }
        out
    }

    /// Whom the Tab list goes to now: everyone each `tab_refresh_seconds`,
    /// and players who just joined.
    pub fn tab_due(&mut self, now: i64, on: &[u128]) -> Vec<u128> {
        if !self.config.tab {
            return Vec::new();
        }
        let all = self
            .tab_at
            .is_none_or(|t| now - t >= self.config.tab_refresh_seconds * 1000);
        if all {
            self.tab_at = Some(now);
        }
        let to: Vec<u128> = on
            .iter()
            .copied()
            .filter(|u| all || !self.tab_sent.contains(u))
            .collect();
        self.tab_sent.extend(to.iter().copied());
        to
    }

    /// The Tab list's header and footer.
    pub fn tab_lines(&self, tps: f64, mspt: f64, players: usize) -> (String, String) {
        let fill = |s: &str| {
            s.replace("{server}", &self.config.server_name)
                .replace("{tps}", &format!("{tps:.1}"))
                .replace("{mspt}", &format!("{mspt:.1}"))
                .replace("{players}", &players.to_string())
        };
        (fill(&self.config.tab_header), fill(&self.config.tab_footer))
    }

    /// `/sidebar [on|off]`: the answer, and the sidebar to show (true) or
    /// take away (false). None for another command.
    pub fn command(&mut self, me: &Player, line: &str) -> Option<(Vec<Out>, Option<bool>)> {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.first() != Some(&"sidebar") {
            return None;
        }
        if !self.config.sidebar {
            return Some((
                vec![Out::Error("The sidebar is off on this server.".into())],
                None,
            ));
        }
        let key = smp_store::uuid_string(me.uuid);
        let show = match words[1..] {
            [] => self.hidden(me.uuid),
            ["on"] => true,
            ["off"] => false,
            _ => {
                return Some((vec![Out::Error("Usage: /sidebar [on|off]".into())], None));
            }
        };
        if show {
            self.changed |= self.data.hidden.remove(&key);
            self.sent.remove(&me.uuid);
        } else {
            self.changed |= self.data.hidden.insert(key);
        }
        let reply = if show {
            "The sidebar is on. /sidebar off hides it."
        } else {
            "The sidebar is off. /sidebar on shows it again."
        };
        Some((vec![Out::Reply(reply.into())], Some(show)))
    }
}

pub const COMMANDS: [&str; 1] = ["sidebar"];

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn dir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("smp-board-{}-{n}", std::process::id()))
    }
    fn me(uuid: u128) -> Player {
        Player {
            uuid,
            name: format!("P{uuid}"),
            level: 0,
        }
    }
    fn accounts(n: u128) -> Vec<(u128, String, i64)> {
        (1..=n)
            .map(|u| (u, format!("P{u}"), u as i64 * 100))
            .collect()
    }

    #[test]
    fn amounts_are_short() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(1_250), "1,250");
        assert_eq!(compact(99_999), "99,999");
        assert_eq!(compact(125_400), "125.4k");
        assert_eq!(compact(3_210_000), "3.2M");
        assert_eq!(compact(7_000_000_000), "7.0B");
    }

    fn me_of(uuid: u128) -> Me {
        Me {
            name: format!("P{uuid}"),
            played_seconds: 7_500,
        }
    }

    #[test]
    fn durations_are_short() {
        assert_eq!(short_duration(0), "0m");
        assert_eq!(short_duration(12 * 60 + 59), "12m");
        assert_eq!(short_duration(7_500), "2h 05m");
    }

    #[test]
    fn the_sidebar_shows_the_player_then_the_three_richest_then_the_address() {
        let d = dir();
        let mut b = Board::open(&d, Config::default()).unwrap();
        assert!(b.due(0, 0));
        b.rebuild(accounts(12), 0, 1);
        let rocky = Me {
            name: "Rocky".into(),
            played_seconds: 7_500,
        };
        let stat = |label, value: &str| Line::Stat {
            label,
            value: value.into(),
        };
        let top = |rank, name: &str, amount: &str, me| Line::Top {
            rank,
            name: name.into(),
            amount: amount.into(),
            me,
        };
        assert_eq!(
            b.lines(11, &rocky, 42),
            vec![
                Line::Name("Rocky".into()),
                stat("Balance", "1,100"),
                stat("Playtime", "2h 05m"),
                stat("Online", "42"),
                Line::Gap,
                Line::Heading,
                top(1, "P12", "1,200", false),
                top(2, "P11", "1,100", true),
                top(3, "P10", "1,000", false),
                Line::Gap,
                Line::Footer("play.playhorizon.dev".into()),
            ]
        );
        // A player with no account yet; a server with no address and no top.
        assert_eq!(b.lines(99, &rocky, 1)[1], stat("Balance", "0"));
        b.config.footer.clear();
        b.config.top = 0;
        assert_eq!(b.lines(99, &rocky, 1).len(), 4);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_old_defaults_move_to_the_new_ones_and_set_ones_stay() {
        let mut old = Config {
            title: "Richest players".into(),
            top: 10,
            ..Config::default()
        };
        assert!(migrate(&mut old));
        assert_eq!(old, Config::default());
        assert!(!migrate(&mut old));
        let mut own = Config {
            title: "My server".into(),
            top: 5,
            ..Config::default()
        };
        assert!(!migrate(&mut own));
        assert_eq!((own.title.as_str(), own.top), ("My server", 5));
    }

    #[test]
    fn sidebars_go_out_on_join_on_a_new_ranking_and_on_the_refresh() {
        let d = dir();
        let mut b = Board::open(&d, Config::default()).unwrap();
        b.rebuild(accounts(3), 0, 1);
        let asked = std::cell::Cell::new(0);
        let me = |u| {
            asked.set(asked.get() + 1);
            me_of(u)
        };
        assert_eq!(b.updates(0, &[1, 2], me).len(), 2);
        // Nothing new within the refresh: nobody is even looked at.
        asked.set(0);
        assert!(b.updates(1_000, &[1, 2], me).is_empty());
        assert_eq!(asked.get(), 0);
        assert!(!b.due(1_000, 1), "no change, not a minute yet");
        assert!(!b.due(1_000, 2), "changed, but within min_seconds");
        assert!(b.due(5_000, 2));
        b.rebuild(accounts(3), 5_000, 2);
        assert!(b.updates(5_000, &[1, 2], me).is_empty(), "same lines");
        // Player 1 gets richer: everyone's top changes.
        let mut a = accounts(3);
        a[0].2 = 1_000;
        b.rebuild(a, 10_000, 3);
        assert_eq!(b.updates(10_000, &[1, 2], me).len(), 2);
        // A newcomer gets theirs at once, and only they are looked at.
        asked.set(0);
        assert_eq!(b.updates(11_000, &[1, 2, 3], me).len(), 1);
        assert_eq!(asked.get(), 1);
        // One who left gets it on return.
        b.updates(12_000, &[2, 3], me);
        assert_eq!(b.updates(13_000, &[1, 2, 3], me).len(), 1);
        // Within the minute player 2 still shows 2 online; the minute's
        // refresh mends it, and sends nothing to the others.
        assert_eq!(b.updates(30_000, &[1, 2, 3], me).len(), 0);
        let sent = b.updates(60_000, &[1, 2, 3], me);
        assert_eq!(sent.iter().map(|s| s.0).collect::<Vec<_>>(), vec![2]);
        assert!(b.due(70_000, 3), "a minute passed");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_hidden_sidebar_stays_hidden_after_a_restart() {
        let d = dir();
        let mut b = Board::open(&d, Config::default()).unwrap();
        b.rebuild(accounts(3), 0, 1);
        let (out, show) = b.command(&me(1), "sidebar off").unwrap();
        assert_eq!(show, Some(false));
        assert!(matches!(&out[0], Out::Reply(s) if s.contains("off")));
        assert_eq!(b.updates(0, &[1, 2], me_of).len(), 1, "only player 2");
        b.save().unwrap();
        let mut b = Board::open(&d, Config::default()).unwrap();
        assert!(b.hidden(1));
        b.rebuild(accounts(3), 0, 1);
        assert_eq!(b.updates(0, &[1], me_of).len(), 0);
        // `/sidebar` toggles it back on, and it is sent at the next update.
        let (_, show) = b.command(&me(1), "sidebar").unwrap();
        assert_eq!(show, Some(true));
        assert_eq!(b.updates(1_000, &[1], me_of).len(), 1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_tab_list_fills_its_placeholders_and_goes_to_newcomers_at_once() {
        let d = dir();
        let mut b = Board::open(&d, Config::default()).unwrap();
        let (h, f) = b.tab_lines(19.96, 6.3, 42);
        assert_eq!(
            (h.as_str(), f.as_str()),
            ("Horizon SMP", "TPS 20.0 · 42 online")
        );
        assert_eq!(b.tab_due(0, &[1, 2]), vec![1, 2]);
        assert!(b.tab_due(1_000, &[1, 2]).is_empty());
        assert_eq!(b.tab_due(2_000, &[1, 2, 3]), vec![3]);
        assert_eq!(b.tab_due(5_000, &[1, 2, 3]), vec![1, 2, 3]);
        let _ = std::fs::remove_dir_all(d);
    }
}
