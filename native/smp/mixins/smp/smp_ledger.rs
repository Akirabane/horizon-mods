// The coin ledger: `audit.log` (smp_economy) already holds one line for every
// coin that moves (who, how much, why, when), appended before the change.
// `/ledger` lets operators of `LEVEL` read it in game: the newest movements,
// one player's, or what the server made and destroyed hour by hour. Only
// the end of the file is read (`TAIL`), from the newest line back, and only
// when an operator asks: nothing on the tick. No game types. Added to the
// game as `game::smp_ledger`; also a module of this crate, for its tests.

use super::smp_economy::{Out, Player};
use super::smp_store;
use serde_json::Value;
use std::path::Path;

/// The operator level that reads the ledger.
pub const LEVEL: u8 = 3;

pub const COMMANDS: [&str; 1] = ["ledger"];

/// The end of `audit.log` a `/ledger` reads (some hours of a busy day).
const TAIL: u64 = 1 << 20;
/// Lines a page.
const PAGE: usize = 10;
/// Hours `/ledger minted` counts.
const HOURS: i64 = 6;
const HOUR_MS: i64 = 3_600_000;

/// `/ledger [page]`, `/ledger <player> [page]`, `/ledger minted`; None for
/// another command. `audit`: the log's path; `find`: a player by name;
/// `name_of`: a UUID's name; `money`: an amount as players read it.
pub fn command(
    me: &Player,
    audit: &Path,
    find: &dyn Fn(&str) -> Option<(u128, String)>,
    name_of: &dyn Fn(u128) -> Option<String>,
    money: &dyn Fn(i64) -> String,
    now: i64,
    line: &str,
) -> Option<Vec<Out>> {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.first() != Some(&"ledger") {
        return None;
    }
    if me.level < LEVEL {
        return Some(vec![Out::Error(
            "Only operators can read the ledger.".into(),
        )]);
    }
    let text = smp_store::tail(audit, TAIL);
    let newest = text
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok());
    let page = |w: Option<&&str>| w.and_then(|p| p.parse::<usize>().ok()).unwrap_or(1).max(1);
    let (who, page) = match words[1..] {
        ["minted"] => {
            let (mut out, covered) = minted(newest, money, now);
            // The end read didn't reach the window's start: the oldest
            // hours may miss movements.
            if !covered && audit.metadata().is_ok_and(|m| m.len() > TAIL) {
                out.insert(
                    1,
                    Out::Reply(
                        "Partial totals: only the newest 1 MiB of the ledger was read; older movements may be missing."
                            .into(),
                    ),
                );
            }
            return Some(out);
        }
        [] => (None, 1),
        [w] if w.parse::<usize>().is_ok() && find(w).is_none() => (None, page(Some(&w))),
        [w, ..] => match find(w) {
            Some(p) => (Some(p), page(words.get(2))),
            None => return Some(vec![Out::Error("No player was found".into())]),
        },
    };
    let key = who.as_ref().map(|(u, _)| smp_store::uuid_string(*u));
    let mine = |e: &Value| {
        key.as_deref()
            .is_none_or(|k| e["from"].as_str() == Some(k) || e["to"].as_str() == Some(k))
    };
    let rows: Vec<String> = newest
        .filter(|e| {
            e["amount"].as_i64().unwrap_or(0) > 0
                && (e.get("from").is_some() || e.get("to").is_some())
        })
        .filter(mine)
        .skip((page - 1).saturating_mul(PAGE))
        .take(PAGE)
        .map(|e| row(&e, name_of, money))
        .collect();
    let whose = who.map_or("Coin movements".to_owned(), |(_, n)| {
        format!("{n}'s coin movements")
    });
    if rows.is_empty() {
        return Some(vec![Out::Reply(format!(
            "{whose}: none in the newest part of the ledger (page {page})."
        ))]);
    }
    let mut out = vec![Out::Reply(format!(
        "{whose}, newest first (page {page}, UTC):"
    ))];
    out.extend(rows.into_iter().map(Out::Reply));
    Some(out)
}

/// `11-05 12:03 pay: Alex → Steve, 300 coins`.
fn row(
    e: &Value,
    name_of: &dyn Fn(u128) -> Option<String>,
    money: &dyn Fn(i64) -> String,
) -> String {
    let player = |side: &str| {
        let key = e[side].as_str()?;
        Some(
            smp_store::parse_uuid(key)
                .and_then(name_of)
                .unwrap_or_else(|| key.to_owned()),
        )
    };
    let pool = e["pool"].as_str().map(|p| format!("the {p} pool"));
    let from = player("from")
        .or_else(|| e["from_pool"].as_str().map(|p| format!("the {p} pool")))
        .or_else(|| pool.clone().filter(|_| e.get("to").is_some()))
        .unwrap_or_else(|| "the server".into());
    let to = player("to")
        .or_else(|| e["to_pool"].as_str().map(|p| format!("the {p} pool")))
        .or_else(|| pool.filter(|_| e.get("from").is_some()))
        .unwrap_or_else(|| "the server".into());
    let when = smp_store::utc_minute(e["time"].as_i64().unwrap_or(0));
    let why = e["reason"]
        .as_str()
        .map(|r| format!(" ({r})"))
        .unwrap_or_default();
    format!(
        "{} {}{why}: {from} → {to}, {}",
        when.get(5..).unwrap_or(&when),
        e["kind"].as_str().unwrap_or("?"),
        money(e["amount"].as_i64().unwrap_or(0))
    )
}

/// `/ledger minted`: what the server made and destroyed in each of the last
/// hours (UTC), and what made most of it; and whether the lines read
/// reached back past the first hour's start.
fn minted(
    newest: impl Iterator<Item = Value>,
    money: &dyn Fn(i64) -> String,
    now: i64,
) -> (Vec<Out>, bool) {
    let first = (now.div_euclid(HOUR_MS) - (HOURS - 1)) * HOUR_MS;
    let mut covered = false;
    let mut hours =
        vec![(0i64, 0i64, std::collections::BTreeMap::<String, i64>::new()); HOURS as usize];
    for e in newest {
        let t = e["time"].as_i64().unwrap_or(0);
        if t < first {
            covered = true;
            break;
        }
        let i = ((t - first) / HOUR_MS).clamp(0, HOURS - 1) as usize;
        let amount = e["amount"].as_i64().unwrap_or(0);
        let kind = e["kind"]
            .as_str()
            .unwrap_or("?")
            .split(' ')
            .next()
            .unwrap_or("?")
            .to_owned();
        match e["flow"].as_str() {
            Some("minted") => {
                hours[i].0 += amount;
                *hours[i].2.entry(kind).or_default() += amount;
            }
            Some("burned") => hours[i].1 += amount,
            _ => {}
        }
    }
    let mut out = vec![Out::Reply(
        "Coins the server made and destroyed, by hour (UTC):".into(),
    )];
    for (i, (made, gone, kinds)) in hours.iter().enumerate().rev() {
        let start = smp_store::utc_minute(first + i as i64 * HOUR_MS);
        let top = kinds
            .iter()
            .max_by_key(|k| k.1)
            .map(|(k, _)| format!(", mostly {k}"))
            .unwrap_or_default();
        out.push(Out::Reply(format!(
            "{}: made {}{top}, destroyed {}",
            start.get(11..).unwrap_or(&start),
            money(*made),
            money(*gone)
        )));
    }
    (out, covered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const ALEX: u128 = 1;
    const STEVE: u128 = 2;

    fn dir() -> std::path::PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("smp-ledger-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn find(name: &str) -> Option<(u128, String)> {
        [(ALEX, "Alex"), (STEVE, "Steve")]
            .into_iter()
            .find(|p| p.1.eq_ignore_ascii_case(name))
            .map(|(u, n)| (u, n.to_owned()))
    }
    fn name_of(u: u128) -> Option<String> {
        [(ALEX, "Alex"), (STEVE, "Steve")]
            .into_iter()
            .find(|p| p.0 == u)
            .map(|p| p.1.to_owned())
    }
    fn money(n: i64) -> String {
        format!("{n} coins")
    }
    fn op() -> Player {
        Player {
            uuid: 9,
            name: "Op".into(),
            level: 3,
        }
    }

    /// The real economy writes the log: what it records is what `/ledger` reads.
    #[test]
    fn the_ledger_reads_back_what_the_economy_wrote() {
        use super::super::smp_economy::{Config, Economy};
        let d = dir();
        let mut e = Economy::open(&d, Config::default()).unwrap();
        e.now = 1_793_880_000_000; // 2026-11-05 12:00 UTC
        e.seen(ALEX, "Alex");
        e.seen(STEVE, "Steve");
        e.adjust("sell 11 minecraft:birch_log", ALEX, 500, "")
            .unwrap();
        e.now += 60_000;
        e.transfer("pay", ALEX, STEVE, 300).unwrap();
        e.now += 60_000;
        e.adjust("buy 1 minecraft:cooked_beef", STEVE, -7, "")
            .unwrap();
        e.save().unwrap();
        let audit = d.join("audit.log");
        let run =
            |line: &str| command(&op(), &audit, &find, &name_of, &money, e.now, line).unwrap();
        assert_eq!(
            run("ledger"),
            vec![
                Out::Reply("Coin movements, newest first (page 1, UTC):".into()),
                Out::Reply(
                    "11-05 12:02 buy 1 minecraft:cooked_beef: Steve → the server, 7 coins".into()
                ),
                Out::Reply("11-05 12:01 pay: Alex → Steve, 300 coins".into()),
                Out::Reply(
                    "11-05 12:00 sell 11 minecraft:birch_log: the server → Alex, 500 coins".into()
                ),
            ]
        );
        let alex = run("ledger alex");
        assert_eq!(alex.len(), 3, "the sale and the payment: {alex:?}");
        assert_eq!(
            alex[0],
            Out::Reply("Alex's coin movements, newest first (page 1, UTC):".into())
        );
        assert!(matches!(&run("ledger alex 2")[0], Out::Reply(s) if s.contains("none")));
        assert_eq!(
            run("ledger nobody"),
            vec![Out::Error("No player was found".into())]
        );
        let minted = run("ledger minted");
        assert_eq!(minted.len(), 1 + HOURS as usize);
        assert_eq!(
            minted[1],
            Out::Reply("12:00: made 500 coins, mostly sell, destroyed 7 coins".into())
        );
        assert_eq!(
            minted[2],
            Out::Reply("11:00: made 0 coins, destroyed 0 coins".into())
        );
        // Not for players.
        let player = Player {
            uuid: ALEX,
            name: "Alex".into(),
            level: 0,
        };
        assert_eq!(
            command(&player, &audit, &find, &name_of, &money, e.now, "ledger"),
            Some(vec![Out::Error(
                "Only operators can read the ledger.".into()
            )])
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn minted_includes_pool_funding_and_starting_balances() {
        use super::super::smp_economy::{Config, Economy};
        let d = dir();
        let mut e = Economy::open(
            &d,
            Config {
                starting_balance: 50,
                ..Config::default()
            },
        )
        .unwrap();
        e.now = 1_793_880_000_000;
        e.seen(ALEX, "Alex");
        e.mint_pool("casino_float", "casino", 100, "the casino's float")
            .unwrap();
        e.save().unwrap();
        let run = || {
            command(
                &op(),
                &d.join("audit.log"),
                &find,
                &name_of,
                &money,
                e.now,
                "ledger minted",
            )
            .unwrap()
        };
        assert!(
            run()
                .iter()
                .any(|o| matches!(o, Out::Reply(s) if s.contains("made 150 coins")))
        );
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(d.join("audit.log"))
            .unwrap();
        file.write_all(&vec![b'\n'; TAIL as usize]).unwrap();
        assert!(
            run()
                .iter()
                .any(|o| matches!(o, Out::Reply(s) if s.starts_with("Partial totals:")))
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_long_ledger_whose_end_covers_the_hours_is_not_partial() {
        let d = dir();
        let audit = d.join("audit.log");
        let now = 1_793_880_000_000i64;
        // 1.5 MiB of yesterday's lines, then this hour's: the end read
        // reaches back past the six hours.
        let old = "{\"amount\":1,\"flow\":\"minted\",\"kind\":\"sell\",\"time\":1793793600000,\"to\":\"00000000-0000-0000-0000-000000000001\"}\n";
        let mut text = old.repeat((3 * TAIL as usize / 2) / old.len());
        text.push_str(
            &old.replace("1793793600000", &now.to_string())
                .replace("\"amount\":1", "\"amount\":42"),
        );
        std::fs::write(&audit, text).unwrap();
        let out = command(&op(), &audit, &find, &name_of, &money, now, "ledger minted").unwrap();
        assert!(
            !out.iter()
                .any(|o| matches!(o, Out::Reply(s) if s.starts_with("Partial"))),
            "{out:?}"
        );
        assert_eq!(
            out[1],
            Out::Reply("12:00: made 42 coins, mostly sell, destroyed 0 coins".into())
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn pools_and_pages_read_as_they_should() {
        let d = dir();
        let audit = d.join("audit.log");
        let mut lines = String::new();
        for i in 0..25 {
            lines.push_str(&format!(
                "{{\"amount\":{},\"from\":\"00000000-0000-0000-0000-000000000001\",\"kind\":\"market_tax\",\"reason\":\"sell 4 minecraft:diamond\",\"time\":{},\"to_pool\":\"treasury\"}}\n",
                i + 1,
                1_793_880_000_000i64 + i * 1000
            ));
        }
        lines.push_str("{\"amount\":0,\"kind\":\"open\",\"time\":1793880100000,\"to\":\"00000000-0000-0000-0000-000000000002\"}\n");
        std::fs::write(&audit, lines).unwrap();
        let run = |line: &str| {
            command(
                &op(),
                &audit,
                &find,
                &name_of,
                &money,
                1_793_880_100_000,
                line,
            )
            .unwrap()
        };
        let first = run("ledger");
        assert_eq!(
            first.len(),
            1 + PAGE,
            "the opened account (0 coins) isn't a movement"
        );
        assert_eq!(
            first[1],
            Out::Reply("11-05 12:00 market_tax (sell 4 minecraft:diamond): Alex → the treasury pool, 25 coins".into())
        );
        let third = run("ledger 3");
        assert_eq!(third.len(), 1 + 5);
        assert!(
            matches!(&third[5], Out::Reply(s) if s.ends_with(", 1 coins")),
            "{third:?}"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_page_number_near_the_largest_is_an_empty_page() {
        let d = dir();
        let audit = d.join("audit.log");
        let mut lines = String::new();
        for i in 0..25 {
            lines.push_str(&format!(
                "{{\"amount\":{},\"from\":\"00000000-0000-0000-0000-000000000001\",\"kind\":\"market_tax\",\"time\":{},\"to_pool\":\"treasury\"}}
",
                i + 1,
                1_793_880_000_000i64 + i * 1000
            ));
        }
        std::fs::write(&audit, lines).unwrap();
        // `usize::MAX / PAGE + 2` is the page whose first row, counted as
        // (page - 1) * PAGE, wraps round to row 4 when it overflows.
        for page in [usize::MAX, usize::MAX / PAGE + 2] {
            let out = command(
                &op(),
                &audit,
                &find,
                &name_of,
                &money,
                1_793_880_100_000,
                &format!("ledger {page}"),
            )
            .unwrap();
            assert_eq!(
                out,
                [Out::Reply(format!(
                    "Coin movements: none in the newest part of the ledger (page {page})."
                ))]
            );
        }
        let _ = std::fs::remove_dir_all(d);
    }
}
