// Games of chance with the in-game currency only (`/coinflip`): a player
// stakes an amount, another matches it, a fair coin decides, the winner
// takes both stakes. Each stake is held in `economy.json` from the moment it
// is offered (`Economy::hold`) until the coin falls or the offer ends, so a
// crash or a restart gives it back and never loses or doubles it. Off by
// default (`gambling.enabled`); the audit log records every
// step. Nothing here is bought with real money or paid out of the game
// (Mojang's usage guidelines). No game types; the coin is the glue's, never
// the world's random. Added to the game as `game::smp_games`; also a module
// of this crate, for its tests.

use super::smp_economy::{Economy, Out, Player, parse_amount};
use serde::{Deserialize, Serialize};

/// `config.json`'s `gambling` section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Off until the owner turns it on.
    pub enabled: bool,
    /// Legacy betting restrictions, ignored.
    pub min_bet: i64,
    pub max_bet: i64,
    /// Legacy setting, ignored: completed games have no cooldown.
    pub cooldown_seconds: i64,
    /// How long an offer waits before its stake goes back.
    pub offer_seconds: i64,
    /// The lottery's ticket price.
    pub ticket_price: i64,
    /// Legacy setting, ignored: no per-player ticket cap.
    pub max_tickets: i64,
    pub lottery_seconds: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            min_bet: 1,
            max_bet: super::smp_economy::MAX_AMOUNT,
            cooldown_seconds: 0,
            offer_seconds: 60,
            ticket_price: 10,
            max_tickets: 0,
            lottery_seconds: 3600,
        }
    }
}

/// An offer waiting for a taker: its stake is held.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub from: u128,
    pub from_name: String,
    /// Only this player may take it; anyone when None.
    pub to: Option<u128>,
    pub amount: i64,
    pub at: i64,
}

#[derive(Default)]
pub struct Games {
    pub config: Config,
    pub offers: Vec<Offer>,
}

const USAGE: &str = "Usage: /coinflip <amount> [player], /coinflip accept <player>, /coinflip cancel, /coinflip list";

impl Games {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }

    /// Offers nobody took in time: their stakes go back.
    pub fn expire(&mut self, economy: &mut Economy, now: i64) -> Vec<Out> {
        let limit = self.config.offer_seconds * 1000;
        let (old, keep): (Vec<Offer>, Vec<Offer>) =
            self.offers.drain(..).partition(|o| now - o.at >= limit);
        self.offers = keep;
        let mut out = Vec::new();
        for o in old {
            // A stake the log refuses to release stays held: given back
            // when the economy next opens.
            if economy
                .release("coinflip_expired", o.from, o.amount)
                .is_ok()
            {
                out.push(Out::Tell(
                    o.from,
                    format!(
                        "Nobody took your coinflip: {} back.",
                        economy.money(o.amount)
                    ),
                ));
            }
        }
        out
    }

    /// The player left: their offer ends and its stake goes back.
    pub fn left(&mut self, economy: &mut Economy, uuid: u128) {
        if let Some(i) = self.offers.iter().position(|o| o.from == uuid) {
            let o = self.offers.remove(i);
            // Refused by the log: still held, given back at the next start.
            let _ = economy.release("coinflip_cancel", o.from, o.amount);
        }
    }

    /// `/coinflip ...`; None for another command. `online` finds a player who
    /// is on by name (uuid, name); `heads` flips the coin.
    pub fn command(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        online: &dyn Fn(&str) -> Option<(u128, String)>,
        now: i64,
        heads: &mut dyn FnMut() -> bool,
        line: &str,
    ) -> Option<Vec<Out>> {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.first() != Some(&"coinflip") {
            return None;
        }
        let err = |s: String| Some(vec![Out::Error(s)]);
        if !self.config.enabled {
            return err("Games of chance are off on this server.".into());
        }
        let mut out = self.expire(economy, now);
        let result = match words[1..] {
            ["list"] => {
                let open: Vec<String> = self
                    .offers
                    .iter()
                    .filter(|o| o.to.is_none_or(|t| t == me.uuid))
                    .map(|o| format!("{}: {}", o.from_name, economy.money(o.amount)))
                    .collect();
                if open.is_empty() {
                    vec![Out::Reply("No coinflip is waiting.".into())]
                } else {
                    vec![Out::Reply(format!(
                        "Coinflips waiting: {}",
                        open.join(", ")
                    ))]
                }
            }
            ["cancel"] => match self.offers.iter().position(|o| o.from == me.uuid) {
                None => vec![Out::Error("You have no coinflip waiting.".into())],
                Some(i) => {
                    let o = self.offers.remove(i);
                    match economy.release("coinflip_cancel", o.from, o.amount) {
                        Ok(()) => vec![Out::Reply(format!(
                            "Coinflip cancelled: {} back.",
                            economy.money(o.amount)
                        ))],
                        Err(e) => {
                            self.offers.insert(i, o);
                            vec![Out::Error(e)]
                        }
                    }
                }
            },
            ["accept", name] => self.accept(economy, me, name, now, heads),
            [amount] | [amount, _] => {
                let to = match words.get(2) {
                    None => None,
                    Some(name) => match online(name) {
                        Some((uuid, _)) if uuid == me.uuid => {
                            return err("You can't flip against yourself.".into());
                        }
                        Some(found) => Some(found),
                        None => return err(format!("{name} is not here.")),
                    },
                };
                match parse_amount(amount) {
                    None => vec![Out::Error(USAGE.into())],
                    Some(a) => self.offer(economy, me, to, a, now),
                }
            }
            _ => vec![Out::Error(USAGE.into())],
        };
        out.extend(result);
        Some(out)
    }

    fn offer(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        to: Option<(u128, String)>,
        amount: i64,
        now: i64,
    ) -> Vec<Out> {
        if self.offers.iter().any(|o| o.from == me.uuid) {
            return vec![Out::Error(
                "You already have a coinflip waiting (/coinflip cancel).".into(),
            )];
        }
        if let Err(e) = economy.hold("coinflip_stake", me.uuid, amount) {
            return vec![Out::Error(e)];
        }
        self.offers.push(Offer {
            from: me.uuid,
            from_name: me.name.clone(),
            to: to.as_ref().map(|(u, _)| *u),
            amount,
            at: now,
        });
        let money = economy.money(amount);
        let secs = self.config.offer_seconds;
        match to {
            Some((uuid, name)) => vec![
                Out::Reply(format!("Coinflip of {money} offered to {name} ({secs} s).")),
                Out::Tell(
                    uuid,
                    format!(
                        "{} offers you a coinflip of {money}: /coinflip accept {}",
                        me.name, me.name
                    ),
                ),
            ],
            None => vec![Out::Reply(format!(
                "Coinflip of {money} offered ({secs} s): /coinflip accept {}",
                me.name
            ))],
        }
    }

    fn accept(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        name: &str,
        _now: i64,
        heads: &mut dyn FnMut() -> bool,
    ) -> Vec<Out> {
        let Some(i) = self.offers.iter().position(|o| {
            o.from_name.eq_ignore_ascii_case(name) && o.to.is_none_or(|t| t == me.uuid)
        }) else {
            return vec![Out::Error(format!("{name} has no coinflip for you."))];
        };
        if self.offers[i].from == me.uuid {
            return vec![Out::Error("You can't flip against yourself.".into())];
        }
        let amount = self.offers[i].amount;
        if let Err(e) = economy.hold("coinflip_stake", me.uuid, amount) {
            return vec![Out::Error(e)];
        }
        let o = self.offers.remove(i);
        let prize = amount * 2;
        let (winner, loser, winner_name, loser_name) = if heads() {
            (o.from, me.uuid, o.from_name.as_str(), me.name.as_str())
        } else {
            (me.uuid, o.from, me.name.as_str(), o.from_name.as_str())
        };
        let stakes = [(o.from, amount), (me.uuid, amount)];
        if let Err(e) = economy.award("coinflip_win", &stakes, winner) {
            // Not decided: both stakes go back (or, failing that, at the next start).
            for (u, a) in stakes {
                let _ = economy.release("coinflip_cancel", u, a);
            }
            return vec![Out::Error(e)];
        }
        let won = economy.money(prize);
        vec![
            Out::Tell(
                winner,
                format!("Coinflip against {loser_name}: you won {won}!"),
            ),
            Out::Tell(
                loser,
                format!(
                    "Coinflip against {winner_name}: you lost {}.",
                    economy.money(amount)
                ),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u128 = 1;
    const B: u128 = 2;

    fn setup(name: &str) -> (Economy, Games, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("smp-games-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Economy::open(&dir, Default::default()).unwrap();
        e.seen(A, "Alex");
        e.seen(B, "Steve");
        e.adjust("eco_give", A, 500, "test").unwrap();
        e.adjust("eco_give", B, 500, "test").unwrap();
        let g = Games::new(Config {
            enabled: true,
            ..Default::default()
        });
        (e, g, dir)
    }
    fn player(uuid: u128) -> Player {
        Player {
            uuid,
            name: if uuid == A { "Alex" } else { "Steve" }.into(),
            level: 0,
        }
    }
    fn online(name: &str) -> Option<(u128, String)> {
        match name.to_ascii_lowercase().as_str() {
            "alex" => Some((A, "Alex".into())),
            "steve" => Some((B, "Steve".into())),
            _ => None,
        }
    }
    fn run(
        e: &mut Economy,
        g: &mut Games,
        who: u128,
        now: i64,
        heads: bool,
        line: &str,
    ) -> Vec<Out> {
        g.command(e, &player(who), &online, now, &mut || heads, line)
            .unwrap()
    }
    fn total(e: &Economy) -> i64 {
        e.balance(A) + e.balance(B) + e.held(A) + e.held(B)
    }

    #[test]
    fn the_stake_is_held_then_the_winner_takes_both() {
        let (mut e, mut g, dir) = setup("win");
        run(&mut e, &mut g, A, 0, true, "coinflip 100");
        assert_eq!((e.balance(A), e.held(A)), (400, 100));
        let out = run(&mut e, &mut g, B, 1000, true, "coinflip accept alex");
        assert!(
            matches!(&out[0], Out::Tell(A, s) if s.contains("you won 200 coins")),
            "{out:?}"
        );
        assert_eq!(
            (e.balance(A), e.balance(B), e.held(A), e.held(B)),
            (600, 400, 0, 0)
        );
        // Tails: the taker wins.
        run(&mut e, &mut g, A, 100_000, false, "coinflip 50 steve");
        run(&mut e, &mut g, B, 101_000, false, "coinflip accept Alex");
        assert_eq!((e.balance(A), e.balance(B)), (550, 450));
        assert_eq!(total(&e), 1000, "nothing made or lost");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn limits_cooldown_and_refusals() {
        let (mut e, mut g, dir) = setup("limits");
        g.config.min_bet = 10;
        g.config.max_bet = 100;
        g.config.cooldown_seconds = 30;
        for line in [
            "coinflip 1001",
            "coinflip 100 alex",
            "coinflip 100 nobody",
            "coinflip x",
        ] {
            assert!(
                matches!(&run(&mut e, &mut g, A, 0, true, line)[0], Out::Error(_)),
                "{line}"
            );
        }
        assert_eq!(total(&e), 1000);
        assert_eq!(e.held(A), 0);
        // More than the balance.
        e.adjust("eco_take", B, -480, "test").unwrap();
        run(&mut e, &mut g, A, 0, true, "coinflip 100");
        assert!(matches!(
            &run(&mut e, &mut g, B, 0, true, "coinflip accept alex")[0],
            Out::Error(_)
        ));
        assert_eq!(e.held(A), 100, "the offer still waits");
        // Only the named player may take a named offer.
        run(&mut e, &mut g, A, 0, true, "coinflip cancel");
        e.adjust("eco_give", B, 480, "test").unwrap();
        // A completed game can be followed immediately by another.
        run(&mut e, &mut g, A, 0, true, "coinflip 10");
        run(&mut e, &mut g, B, 0, true, "coinflip accept alex");
        assert!(matches!(
            &run(&mut e, &mut g, A, 0, true, "coinflip 5")[0],
            Out::Reply(_)
        ));
        run(&mut e, &mut g, A, 0, true, "coinflip cancel");
        assert!(matches!(
            &run(&mut e, &mut g, A, 30_000, true, "coinflip 10")[0],
            Out::Reply(_)
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn off_by_default() {
        let (mut e, _, dir) = setup("off");
        let mut g = Games::default();
        assert!(
            matches!(&run(&mut e, &mut g, A, 0, true, "coinflip 100")[0], Out::Error(s) if s.contains("off"))
        );
        assert_eq!(e.held(A), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_way_out_gives_the_stake_back() {
        let (mut e, mut g, dir) = setup("out");
        run(&mut e, &mut g, A, 0, true, "coinflip 100");
        run(&mut e, &mut g, A, 0, true, "coinflip cancel");
        assert_eq!((e.balance(A), e.held(A)), (500, 0));
        run(&mut e, &mut g, A, 0, true, "coinflip 100");
        let out = g.expire(&mut e, 60_000);
        assert!(matches!(&out[0], Out::Tell(A, _)));
        assert_eq!((e.balance(A), e.held(A)), (500, 0));
        run(&mut e, &mut g, A, 0, true, "coinflip 100");
        g.left(&mut e, A);
        assert_eq!((e.balance(A), e.held(A)), (500, 0));
        // A crash with a stake held: saved, then opened again, it goes back.
        run(&mut e, &mut g, A, 0, true, "coinflip 100");
        e.save().unwrap();
        let again = Economy::open(&dir, Default::default()).unwrap();
        assert_eq!((again.balance(A), again.held(A)), (500, 0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
