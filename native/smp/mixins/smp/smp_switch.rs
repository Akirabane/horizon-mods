// The SMP's emergency switches: an operator of level 3 turns parts of the SMP
// off (`/smp off [part] [reason]`) and on again (`/smp on [part]`) while the
// server runs, kept in `switches.json` so a restart keeps them off. A part
// that is off takes no command and no menu click, and its timed work (daily
// rewards, interest, contracts, taxes, auctions ending, futures marking, the
// lottery's draw) waits; nothing held is lost: it goes on when the part is on
// again. `/smp off` alone turns off every part but land protection, which an
// operator turns off by name (`/smp off claims`). No game types. Added to the
// game as `game::smp_switch`; also a module of this crate, for its tests.

use super::smp_economy::{Out, Player};
use super::smp_store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The operator level that turns parts off and on.
pub const LEVEL: u8 = 3;

pub const COMMANDS: [&str; 1] = ["smp"];

/// A part of the SMP that can be turned off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Part {
    /// Balances, `/pay`, `/eco`, the Treasury, taxes, the bank, contracts and
    /// the daily reward.
    Economy,
    /// `/worth`, `/sell`, `/buy`, `/shop`, `/market`.
    Market,
    /// `/ah`.
    Auction,
    /// `/trade`.
    Trades,
    /// `/coinflip`, `/lottery`, `/casino`.
    Gambling,
    /// `/futures`.
    Futures,
    /// Spawn, homes, warps, `/tpa`, `/back`.
    Places,
    /// Land protection and `/claim`.
    Claims,
    /// The sidebar and the Tab list's header and footer.
    Board,
    /// Welcomes, AFK and the commands' rate limit.
    Presence,
    /// Nicknames.
    Nicks,
    /// `/invsee`, `/endersee`.
    Invsee,
}

impl Part {
    pub const ALL: [Part; 12] = [
        Part::Economy,
        Part::Market,
        Part::Auction,
        Part::Trades,
        Part::Gambling,
        Part::Futures,
        Part::Places,
        Part::Claims,
        Part::Board,
        Part::Presence,
        Part::Nicks,
        Part::Invsee,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Part::Economy => "economy",
            Part::Market => "market",
            Part::Auction => "auction",
            Part::Trades => "trades",
            Part::Gambling => "gambling",
            Part::Futures => "futures",
            Part::Places => "places",
            Part::Claims => "claims",
            Part::Board => "board",
            Part::Presence => "presence",
            Part::Nicks => "nicks",
            Part::Invsee => "invsee",
        }
    }

    pub fn named(name: &str) -> Option<Part> {
        Part::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(name))
    }

    /// The part a command belongs to (its first word); None for `/smp` and
    /// what isn't the SMP's.
    pub fn of_command(word: &str) -> Option<Part> {
        Some(match word {
            "balance" | "bal" | "pay" | "baltop" | "eco" | "money" | "treasury" | "taxes"
            | "bank" | "jobs" => Part::Economy,
            "worth" | "sell" | "buy" | "shop" | "market" => Part::Market,
            "ah" => Part::Auction,
            "trade" => Part::Trades,
            "coinflip" | "lottery" | "casino" => Part::Gambling,
            "futures" => Part::Futures,
            "claim" | "unclaim" | "claims" | "trust" | "untrust" => Part::Claims,
            "sidebar" => Part::Board,
            "afk" => Part::Presence,
            "nick" | "realname" => Part::Nicks,
            "invsee" | "endersee" => Part::Invsee,
            "spawn" | "setspawn" | "home" | "sethome" | "delhome" | "homes" | "warp" | "warps"
            | "setwarp" | "delwarp" | "tpa" | "tpaccept" | "tpdeny" | "rules" | "back"
            | "playtime" => Part::Places,
            _ => return None,
        })
    }
}

/// Why and by whom a part is off.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Off {
    pub by: String,
    /// When (ms).
    pub at: i64,
    pub reason: String,
}

/// `switches.json`: the parts that are off, by name.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    off: BTreeMap<String, Off>,
}

pub struct Switches {
    path: PathBuf,
    data: Data,
    changed: bool,
}

/// What a `/smp` command changed, for the glue to carry out.
#[derive(Debug, Default, PartialEq)]
pub struct Changed {
    pub off: Vec<Part>,
    pub on: Vec<Part>,
}

impl Switches {
    /// `switches.json` in `dir`. A file that can't be read turns nothing off
    /// (and is logged by the caller): the switches are for emergencies, not a
    /// reason for the SMP to stop.
    pub fn open(dir: &Path) -> (Self, Option<String>) {
        let path = dir.join("switches.json");
        let (data, problem) = match smp_store::read_json(&path) {
            Ok(d) => (d, None),
            Err(e) => (Data::default(), Some(e)),
        };
        let mut s = Self {
            path,
            data,
            changed: false,
        };
        // Names of parts this version doesn't know are dropped.
        let before = s.data.off.len();
        s.data.off.retain(|name, _| Part::named(name).is_some());
        s.changed = s.data.off.len() != before;
        (s, problem)
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.changed {
            smp_store::write_json(&self.path, &self.data)?;
            self.changed = false;
        }
        Ok(())
    }

    pub fn on(&self, part: Part) -> bool {
        !self.data.off.contains_key(part.name())
    }

    /// Why a part is off, as the operator said.
    pub fn why(&self, part: Part) -> Option<&str> {
        self.data.off.get(part.name()).map(|o| o.reason.as_str())
    }

    /// `/smp`, `/smp status`, `/smp off [part|all] [reason]`,
    /// `/smp on [part|all]`: the answer and what changed. None for another
    /// command.
    pub fn command(&mut self, me: &Player, now: i64, line: &str) -> Option<(Vec<Out>, Changed)> {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.first() != Some(&"smp") {
            return None;
        }
        let none = Changed::default();
        if me.level < LEVEL {
            return Some((
                vec![Out::Error(
                    "Only operators turn the SMP's parts off and on.".into(),
                )],
                none,
            ));
        }
        let usage = || {
            Out::Error(format!(
                "Usage: /smp status, /smp off [part|all] [reason], /smp on [part|all]. Parts: {}.",
                Part::ALL.map(Part::name).join(", ")
            ))
        };
        let (turn_off, rest) = match words[1..] {
            [] | ["status"] => return Some((self.status(), none)),
            ["off", ref rest @ ..] => (true, rest),
            ["on", ref rest @ ..] => (false, rest),
            _ => return Some((vec![usage()], none)),
        };
        // The part, or every one: `all`, or none named.
        let (parts, reason): (Vec<Part>, &[&str]) = match rest.first() {
            None => (everything(turn_off), &[]),
            Some(w) if w.eq_ignore_ascii_case("all") => (everything(turn_off), &rest[1..]),
            Some(w) => match Part::named(w) {
                Some(p) => (vec![p], &rest[1..]),
                // `/smp off <reason>` turns off everything (but claims).
                None if turn_off => (everything(true), rest),
                None => return Some((vec![usage()], none)),
            },
        };
        let mut changed = Changed::default();
        for p in parts {
            if turn_off && self.on(p) {
                let off = Off {
                    by: me.name.clone(),
                    at: now,
                    reason: reason.join(" "),
                };
                self.data.off.insert(p.name().into(), off);
                changed.off.push(p);
            } else if !turn_off && self.data.off.remove(p.name()).is_some() {
                changed.on.push(p);
            }
        }
        self.changed |= !changed.off.is_empty() || !changed.on.is_empty();
        let names = |ps: &[Part]| ps.iter().map(|p| p.name()).collect::<Vec<_>>().join(", ");
        let out = match (turn_off, changed.off.is_empty() && changed.on.is_empty()) {
            (true, true) => vec![Out::Reply("Already off.".into())],
            (false, true) => vec![Out::Reply("Already on.".into())],
            (true, false) => {
                let why = if reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", reason.join(" "))
                };
                let mut out = vec![Out::Reply(format!(
                    "Turned off: {}{why}. /smp on turns them on again.",
                    names(&changed.off)
                ))];
                if !self.on(Part::Claims) || changed.off.contains(&Part::Claims) {
                    out.push(Out::Reply(
                        "Land is not protected while claims are off.".into(),
                    ));
                } else {
                    out.push(Out::Reply(
                        "Land protection stays on (/smp off claims turns it off too).".into(),
                    ));
                }
                out
            }
            (false, false) => vec![Out::Reply(format!(
                "Turned on again: {}.",
                names(&changed.on)
            ))],
        };
        Some((out, changed))
    }

    /// What is off, by whom, since when and why.
    pub fn status(&self) -> Vec<Out> {
        if self.data.off.is_empty() {
            return vec![Out::Reply("Every part of the SMP is on.".into())];
        }
        let mut out = vec![Out::Reply(format!(
            "Off: {} of {} parts.",
            self.data.off.len(),
            Part::ALL.len()
        ))];
        for (name, off) in &self.data.off {
            let when = utc_minute(off.at);
            let why = if off.reason.is_empty() {
                String::new()
            } else {
                format!(": {}", off.reason)
            };
            out.push(Out::Reply(format!(
                "- {name}, by {} at {when} UTC{why}",
                off.by
            )));
        }
        out
    }
}

/// The answer to a command of a part that is off.
pub fn refusal(part: Part) -> Out {
    Out::Error(format!(
        "{} is turned off for now by the server's staff.",
        match part {
            Part::Economy => "The economy",
            Part::Market => "The market",
            Part::Auction => "The auction house",
            Part::Trades => "Trading",
            Part::Gambling => "Gambling",
            Part::Futures => "Futures",
            Part::Places => "Teleporting",
            Part::Claims => "Claiming",
            Part::Board => "The sidebar",
            Part::Presence => "AFK",
            Part::Nicks => "Nicknames",
            Part::Invsee => "Inventory viewing",
        }
    ))
}

/// Every part, but land protection when turning off.
fn everything(turn_off: bool) -> Vec<Part> {
    Part::ALL
        .into_iter()
        .filter(|&p| !turn_off || p != Part::Claims)
        .collect()
}

/// `2026-11-02 14:05` from ms since the epoch (UTC).
fn utc_minute(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn dir() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("smp-switch-{}-{n}", std::process::id()))
    }
    fn op() -> Player {
        Player {
            uuid: 1,
            name: "luzW".into(),
            level: 3,
        }
    }
    fn texts(out: &[Out]) -> String {
        out.iter()
            .map(|o| match o {
                Out::Reply(s) | Out::Error(s) | Out::Tell(_, s) | Out::All(s) => s.as_str(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_command_of_the_smp_belongs_to_a_part() {
        // The glue's own (its `COMMANDS`), then each part's.
        let glue = [
            "balance", "bal", "pay", "baltop", "eco", "money", "worth", "sell", "buy", "shop",
            "market", "trade", "coinflip", "lottery", "treasury", "taxes", "ah", "bank", "jobs",
            "casino", "futures",
        ];
        let parts = [
            &super::super::smp_places::COMMANDS[..],
            &super::super::smp_claims::COMMANDS,
            &super::super::smp_board::COMMANDS,
            &super::super::smp_presence::COMMANDS,
            &super::super::smp_nick::COMMANDS,
            &super::super::smp_invsee::COMMANDS,
        ];
        for word in glue.into_iter().chain(parts.into_iter().flatten().copied()) {
            assert!(Part::of_command(word).is_some(), "{word}");
        }
        assert_eq!(
            Part::of_command("smp"),
            None,
            "the switch itself is never off"
        );
        assert_eq!(
            Part::of_command("tp"),
            None,
            "vanilla's commands are not the SMP's"
        );
    }

    #[test]
    fn off_alone_turns_off_everything_but_claims_and_survives_a_restart() {
        let d = dir();
        let (mut s, problem) = Switches::open(&d);
        assert!(problem.is_none());
        let (out, changed) = s
            .command(&op(), 1_762_092_300_000, "smp off market exploit")
            .unwrap();
        assert_eq!(changed.off, vec![Part::Market], "a named part alone");
        assert!(
            texts(&out).contains("Turned off: market (exploit)"),
            "{}",
            texts(&out)
        );
        let (out, changed) = s
            .command(&op(), 1_762_092_300_000, "smp off duping bug")
            .unwrap();
        assert_eq!(
            changed.off.len(),
            Part::ALL.len() - 2,
            "all but claims and the market already off"
        );
        assert!(!changed.off.contains(&Part::Claims));
        assert!(
            texts(&out).contains("Land protection stays on"),
            "{}",
            texts(&out)
        );
        assert!(s.on(Part::Claims) && !s.on(Part::Economy) && !s.on(Part::Invsee));
        s.save().unwrap();
        let (s, _) = Switches::open(&d);
        assert!(!s.on(Part::Market) && !s.on(Part::Gambling) && s.on(Part::Claims));
        let status = texts(&s.status());
        assert!(
            status.contains("- market, by luzW at 2025-11-02 14:05 UTC: exploit"),
            "{status}"
        );
        assert!(
            status.contains("- economy, by luzW at 2025-11-02 14:05 UTC: duping bug"),
            "{status}"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn claims_go_off_only_by_name_and_on_with_the_rest() {
        let d = dir();
        let (mut s, _) = Switches::open(&d);
        s.command(&op(), 0, "smp off all");
        assert!(s.on(Part::Claims));
        let (out, changed) = s.command(&op(), 0, "smp off claims grief").unwrap();
        assert_eq!(changed.off, vec![Part::Claims]);
        assert!(
            texts(&out).contains("Land is not protected"),
            "{}",
            texts(&out)
        );
        let (_, changed) = s.command(&op(), 0, "smp on").unwrap();
        assert_eq!(changed.on.len(), Part::ALL.len());
        assert!(Part::ALL.iter().all(|&p| s.on(p)));
        let (out, changed) = s.command(&op(), 0, "smp on market").unwrap();
        assert_eq!(changed, Changed::default());
        assert_eq!(texts(&out), "Already on.");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn players_below_level_three_change_nothing() {
        let d = dir();
        let (mut s, _) = Switches::open(&d);
        let player = Player {
            uuid: 2,
            name: "Steve".into(),
            level: 2,
        };
        let (out, changed) = s.command(&player, 0, "smp off").unwrap();
        assert_eq!(changed, Changed::default());
        assert!(matches!(&out[0], Out::Error(_)));
        assert!(Part::ALL.iter().all(|&p| s.on(p)));
        assert!(s.command(&player, 0, "balance").is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_damaged_file_turns_nothing_off_and_is_reported() {
        let d = dir();
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("switches.json"), "{ not json").unwrap();
        let (s, problem) = Switches::open(&d);
        assert!(problem.is_some());
        assert!(Part::ALL.iter().all(|&p| s.on(p)));
        // A part this version doesn't know is dropped.
        std::fs::write(
            d.join("switches.json"),
            r#"{"off": {"rockets": {}, "market": {"by": "x"}}}"#,
        )
        .unwrap();
        let (s, _) = Switches::open(&d);
        assert!(!s.on(Part::Market));
        assert_eq!(s.data.off.len(), 1);
        let _ = std::fs::remove_dir_all(d);
    }
}
