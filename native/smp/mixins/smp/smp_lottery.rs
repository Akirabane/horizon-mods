// The lottery (`/lottery`): players buy tickets with the in-game currency;
// once `lottery_seconds` have passed since a round's first ticket, one ticket
// is drawn at random and its owner takes the pot. The tickets' money is held
// in `economy.json` (`Economy::hold`) until the draw, so a crash or a restart
// gives every ticket back and never loses or doubles the pot. A round with a
// single player is given back too: there is nobody to win from. Part of
// `gambling` (off by default); nothing here is bought with real money or
// paid out of the game. No game types; the draw is the glue's random, never
// the world's. Added to the game as `game::smp_lottery`; also a module of
// this crate, for its tests.

use super::smp_economy::{Economy, Out, Player, parse_amount};
use super::smp_games::Config;

/// The round being sold.
#[derive(Default)]
pub struct Lottery {
    /// Tickets by player, in the order they first bought: (uuid, name, tickets).
    pub tickets: Vec<(u128, String, i64)>,
    /// When the round draws (ms); None until its first ticket.
    pub draw_at: Option<i64>,
}

impl Lottery {
    fn pot(&self, price: i64) -> i64 {
        self.tickets.iter().map(|t| t.2).sum::<i64>() * price
    }

    /// `/lottery` and `/lottery buy <tickets>`; None for another command.
    pub fn command(
        &mut self,
        config: &Config,
        economy: &mut Economy,
        me: &Player,
        now: i64,
        line: &str,
    ) -> Option<Vec<Out>> {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.first() != Some(&"lottery") {
            return None;
        }
        if !config.enabled {
            return Some(vec![Out::Error(
                "Games of chance are off on this server.".into(),
            )]);
        }
        let price = config.ticket_price.max(1);
        let mine = self
            .tickets
            .iter()
            .find(|t| t.0 == me.uuid)
            .map_or(0, |t| t.2);
        Some(match words[1..] {
            [] => {
                let when = match self.draw_at {
                    Some(t) => format!("draws in {} s", ((t - now).max(0) + 999) / 1000),
                    None => "draws once tickets are bought".into(),
                };
                vec![Out::Reply(format!(
                    "Lottery: pot {}, {} ticket(s) of yours, {when}. A ticket costs {}: /lottery buy <tickets>",
                    economy.money(self.pot(price)),
                    mine,
                    economy.money(price)
                ))]
            }
            ["buy", n] => match parse_amount(n) {
                None => vec![Out::Error("Usage: /lottery buy <tickets>".into())],
                Some(n) => match n
                    .checked_mul(price)
                    .ok_or_else(|| "Ticket cost is too large.".to_string())
                    .and_then(|cost| {
                        self.pot(price).checked_add(cost).ok_or_else(|| {
                            "Lottery pot exceeds the supported numeric range.".to_string()
                        })?;
                        economy.hold("lottery_ticket", me.uuid, cost)
                    }) {
                    Err(e) => vec![Out::Error(e)],
                    Ok(()) => {
                        match self.tickets.iter_mut().find(|t| t.0 == me.uuid) {
                            Some(t) => t.2 += n,
                            None => self.tickets.push((me.uuid, me.name.clone(), n)),
                        }
                        let draw = *self
                            .draw_at
                            .get_or_insert(now + config.lottery_seconds * 1000);
                        vec![Out::Reply(format!(
                            "You bought {n} ticket(s) for {}; the pot is {}, drawn in {} s.",
                            economy.money(n * price),
                            economy.money(self.pot(price)),
                            ((draw - now).max(0) + 999) / 1000
                        ))]
                    }
                },
            },
            _ => vec![Out::Error("Usage: /lottery, /lottery buy <tickets>".into())],
        })
    }

    /// Draws the round when its time has come: `pick(n)` is a random number
    /// below `n`. Nothing before then.
    pub fn tick(
        &mut self,
        config: &Config,
        economy: &mut Economy,
        now: i64,
        pick: &mut dyn FnMut(u64) -> u64,
    ) -> Vec<Out> {
        if self.draw_at.is_none_or(|t| now < t) {
            return Vec::new();
        }
        let price = config.ticket_price.max(1);
        let round = std::mem::take(self);
        let stakes: Vec<(u128, i64)> = round.tickets.iter().map(|t| (t.0, t.2 * price)).collect();
        if round.tickets.len() < 2 {
            // Nobody to win from: the tickets go back.
            for (uuid, amount) in &stakes {
                let _ = economy.release("lottery_refund", *uuid, *amount);
            }
            return round
                .tickets
                .iter()
                .map(|t| {
                    Out::Tell(
                        t.0,
                        "The lottery had no other player: your tickets were given back.".into(),
                    )
                })
                .collect();
        }
        let total: i64 = round.tickets.iter().map(|t| t.2).sum();
        let mut ticket = pick(total as u64) as i64;
        let winner = round
            .tickets
            .iter()
            .find(|t| {
                ticket -= t.2;
                ticket < 0
            })
            .expect("the ticket is below the total");
        let prize = total * price;
        match economy.award("lottery_win", &stakes, winner.0) {
            Ok(()) => vec![Out::All(format!(
                "Lottery: {} won {} with {} of {total} tickets!",
                winner.1,
                economy.money(prize),
                winner.2
            ))],
            Err(_) => {
                // Not drawn: the tickets go back (or at the next start).
                for (uuid, amount) in &stakes {
                    let _ = economy.release("lottery_refund", *uuid, *amount);
                }
                vec![Out::All(
                    "The lottery couldn't be drawn: every ticket was given back.".into(),
                )]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u128 = 1;
    const B: u128 = 2;

    fn setup(name: &str) -> (Economy, Config, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("smp-lottery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Economy::open(&dir, Default::default()).unwrap();
        e.seen(A, "Alex");
        e.seen(B, "Steve");
        e.adjust("eco_give", A, 500, "test").unwrap();
        e.adjust("eco_give", B, 500, "test").unwrap();
        let c = Config {
            enabled: true,
            ..Default::default()
        };
        (e, c, dir)
    }
    fn player(uuid: u128) -> Player {
        Player {
            uuid,
            name: if uuid == A { "Alex" } else { "Steve" }.into(),
            level: 0,
        }
    }

    #[test]
    fn tickets_are_held_and_one_is_drawn_for_the_pot() {
        let (mut e, c, dir) = setup("draw");
        let mut l = Lottery::default();
        l.command(&c, &mut e, &player(A), 0, "lottery buy 3")
            .unwrap();
        l.command(&c, &mut e, &player(B), 1000, "lottery buy 1")
            .unwrap();
        assert_eq!(
            (e.balance(A), e.held(A), e.balance(B), e.held(B)),
            (470, 30, 490, 10)
        );
        // Not yet: the round draws an hour after its first ticket.
        assert!(l.tick(&c, &mut e, 3_599_999, &mut |_| 0).is_empty());
        // Ticket 3 (the fourth) is Steve's.
        let out = l.tick(&c, &mut e, 3_600_000, &mut |n| {
            assert_eq!(n, 4);
            3
        });
        assert!(
            matches!(&out[0], Out::All(s) if s.contains("Steve won 40 coins with 1 of 4")),
            "{out:?}"
        );
        assert_eq!(
            (e.balance(A), e.balance(B), e.held(A), e.held(B)),
            (470, 530, 0, 0)
        );
        assert!(l.tickets.is_empty() && l.draw_at.is_none(), "a new round");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_round_of_one_player_is_given_back() {
        let (mut e, c, dir) = setup("alone");
        let mut l = Lottery::default();
        l.command(&c, &mut e, &player(A), 0, "lottery buy 5")
            .unwrap();
        l.tick(&c, &mut e, 3_600_000, &mut |_| 0);
        assert_eq!((e.balance(A), e.held(A)), (500, 0));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unlimited_tickets_still_require_funds_and_enabled_gambling() {
        let (mut e, c, dir) = setup("limits");
        let mut l = Lottery::default();
        let out = l
            .command(&c, &mut e, &player(A), 0, "lottery buy 101")
            .unwrap();
        assert!(matches!(&out[0], Out::Error(_)), "insufficient funds");
        e.adjust("test", A, 2000, "").unwrap();
        let out = l
            .command(&c, &mut e, &player(A), 0, "lottery buy 101")
            .unwrap();
        assert!(matches!(&out[0], Out::Reply(_)), "{out:?}");
        assert_eq!(e.held(A), 1010);
        e.adjust("test", A, -990, "").unwrap();
        let out = l
            .command(&c, &mut e, &player(A), 0, "lottery buy 51")
            .unwrap();
        assert!(
            matches!(&out[0], Out::Error(_)),
            "510 coins is more than 500"
        );
        assert_eq!(e.held(A), 1010);
        let off = Config::default();
        let out = l
            .command(&off, &mut e, &player(A), 0, "lottery buy 1")
            .unwrap();
        assert!(matches!(&out[0], Out::Error(s) if s.contains("off")));
        // A restart gives held tickets back.
        l.command(&c, &mut e, &player(A), 0, "lottery buy 2")
            .unwrap();
        e.save().unwrap();
        let again = Economy::open(&dir, Default::default()).unwrap();
        assert_eq!((again.balance(A), again.held(A)), (1510, 0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
