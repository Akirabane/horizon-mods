// The casino (`/casino`): games against the house played in chest menus that
// animate (a roulette wheel that turns and slows down, slot reels that
// scroll), with sounds, titles and fireworks, all for an unmodded client.
// This file holds what every game shares: the settings, the server's random
// (ChaCha20 seeded from the system, never the world's or anything a client
// can see), the bets (held in `economy.json` from the moment they are placed,
// so a crash gives them back), the day's ledger (`casino.json`: what
// each player staked and got back today, the day's biggest wins, each game's
// totals) and the lobby menu. Winnings are paid from the casino's own pot
// (the economy's "casino" pool, which every lost bet fills); a win the pot
// can't cover is paid up to what it holds. A new casino's pot gets its
// float once, made for it; then it keeps the house edge, a money sink for
// the players. Each game is a file of its
// own (smp_casino_roulette, ...). No game types: the glue (casino_glue.rs)
// turns the icons and effects into menus, packets and commands. Added to the
// game as `game::smp_casino`; also a module of this crate, for its tests.

use super::smp_casino_blackjack::{self as bj, Blackjack};
use super::smp_casino_crash::{self as crash, Crash};
use super::smp_casino_roulette::{self as roulette, Roulette};
use super::smp_casino_slots::{self as slots, Slots};
use super::smp_economy::{Economy, Out, Player};
use super::smp_store::{read_json, uuid_string, write_json};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The economy's pool the casino pays from and its lost bets go to.
pub const POT: &str = "casino";
/// Every casino menu: 6 rows.
pub const ROWS: u8 = 6;
pub const SIZE: usize = 54;
/// The least time between two pictures of an animated menu: 5 a second.
pub const FRAME_MS: i64 = 200;
const DAY_MS: i64 = 86_400_000;
/// Wins kept in the day's list.
const WINS: usize = 10;

/// `config.json`'s `casino` section. The casino also needs `gambling.enabled`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    /// Legacy betting restrictions, ignored (positive amounts are required).
    pub min_bet: i64,
    pub max_bet: i64,
    pub daily_loss_cap: i64,
    pub daily_win_cap: i64,
    pub cooldown_ms: i64,
    /// What a new casino's pot starts with, for paying wins (made once).
    pub float: i64,
    /// A win of at least this profit is told to everyone.
    pub announce_profit: i64,
    /// A win of at least this many times the stake is a jackpot: fireworks,
    /// everyone told, and the Discord bridge.
    pub jackpot_multiple: i64,
    /// Crash: the house's edge, and the highest multiplier a flight reaches.
    pub crash_edge_percent: i64,
    pub crash_max: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            min_bet: 1,
            max_bet: super::smp_economy::MAX_AMOUNT,
            daily_loss_cap: 0,
            daily_win_cap: 0,
            cooldown_ms: 0,
            float: 20_000,
            announce_profit: 5000,
            jackpot_multiple: 50,
            crash_edge_percent: 4,
            crash_max: 100,
        }
    }
}

/// The games, as the ledger and the audit log name them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Game {
    Roulette,
    Slots,
    Crash,
    Blackjack,
}

impl Game {
    pub fn id(self) -> &'static str {
        match self {
            Game::Roulette => "roulette",
            Game::Slots => "slots",
            Game::Crash => "crash",
            Game::Blackjack => "blackjack",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Game::Roulette => "Roulette",
            Game::Slots => "Slots",
            Game::Crash => "Crash",
            Game::Blackjack => "Blackjack",
        }
    }
}

/// A random number generator for the games: ChaCha20 (RFC 8439's block
/// function) keyed from the system's randomness, so no draw can be predicted
/// from what players see, nor from the world's seed or the time.
pub struct Rng {
    key: [u32; 8],
    counter: u64,
    block: [u32; 16],
    used: usize,
}

/// One ChaCha20 block: the key, then words 12 to 15 (counter and nonce).
fn chacha20(key: &[u32; 8], tail: [u32; 4]) -> [u32; 16] {
    let mut s = [0u32; 16];
    s[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    s[4..12].copy_from_slice(key);
    s[12..].copy_from_slice(&tail);
    let mut x = s;
    fn quarter(x: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        x[a] = x[a].wrapping_add(x[b]);
        x[d] = (x[d] ^ x[a]).rotate_left(16);
        x[c] = x[c].wrapping_add(x[d]);
        x[b] = (x[b] ^ x[c]).rotate_left(12);
        x[a] = x[a].wrapping_add(x[b]);
        x[d] = (x[d] ^ x[a]).rotate_left(8);
        x[c] = x[c].wrapping_add(x[d]);
        x[b] = (x[b] ^ x[c]).rotate_left(7);
    }
    for _ in 0..10 {
        quarter(&mut x, 0, 4, 8, 12);
        quarter(&mut x, 1, 5, 9, 13);
        quarter(&mut x, 2, 6, 10, 14);
        quarter(&mut x, 3, 7, 11, 15);
        quarter(&mut x, 0, 5, 10, 15);
        quarter(&mut x, 1, 6, 11, 12);
        quarter(&mut x, 2, 7, 8, 13);
        quarter(&mut x, 3, 4, 9, 14);
    }
    for (o, i) in x.iter_mut().zip(s) {
        *o = o.wrapping_add(i);
    }
    x
}

impl Rng {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let mut key = [0u32; 8];
        for (k, c) in key.iter_mut().zip(seed.as_chunks::<4>().0) {
            *k = u32::from_le_bytes(*c);
        }
        Self {
            key,
            counter: 0,
            block: [0; 16],
            used: 16,
        }
    }
    /// Keyed from `/dev/urandom`; where there is none, from the standard
    /// library's own random hash keys (also seeded by the system).
    pub fn from_os() -> Self {
        use std::io::Read;
        let mut seed = [0u8; 32];
        let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed));
        if read.is_err() {
            use std::hash::BuildHasher;
            for chunk in seed.as_chunks_mut::<8>().0 {
                let h = std::collections::hash_map::RandomState::new()
                    .hash_one(std::time::SystemTime::now());
                chunk.copy_from_slice(&h.to_le_bytes());
            }
        }
        Self::from_seed(seed)
    }
    pub fn next_u32(&mut self) -> u32 {
        if self.used == 16 {
            let c = self.counter;
            self.block = chacha20(&self.key, [c as u32, (c >> 32) as u32, 0, 0]);
            self.counter += 1;
            self.used = 0;
        }
        self.used += 1;
        self.block[self.used - 1]
    }
    pub fn next_u64(&mut self) -> u64 {
        u64::from(self.next_u32()) | (u64::from(self.next_u32()) << 32)
    }
    /// A uniform number below `n` (n > 0), without modulo bias.
    pub fn below(&mut self, n: u64) -> u64 {
        let n = n.max(1);
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }
}

/// A colour, for menu names and chat lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    White,
    Gray,
    DarkGray,
    Gold,
    Yellow,
    Green,
    Red,
    Aqua,
}

impl Color {
    pub fn name(self) -> &'static str {
        match self {
            Color::White => "white",
            Color::Gray => "gray",
            Color::DarkGray => "dark_gray",
            Color::Gold => "gold",
            Color::Yellow => "yellow",
            Color::Green => "green",
            Color::Red => "red",
            Color::Aqua => "aqua",
        }
    }
}

/// A menu cell: an item named in a colour, `count` of it (the number shown).
#[derive(Clone, Debug, PartialEq)]
pub struct Icon {
    pub item: &'static str,
    pub name: String,
    pub color: Color,
    pub count: u8,
}

pub fn icon(item: &'static str, name: impl Into<String>, color: Color) -> Option<Icon> {
    Some(Icon {
        item,
        name: name.into(),
        color,
        count: 1,
    })
}

pub fn filler(item: &'static str) -> Option<Icon> {
    icon(item, " ", Color::DarkGray)
}

/// What the glue does for the casino: lines, sounds, titles, fireworks.
#[derive(Clone, Debug, PartialEq)]
pub enum Fx {
    Out(Out),
    /// A sound at the player, for them only.
    Sound {
        to: u128,
        sound: &'static str,
        volume: f32,
        pitch: f32,
    },
    /// The big title and its subtitle.
    Title {
        to: u128,
        title: String,
        subtitle: String,
        color: Color,
    },
    /// The line over the hotbar.
    Bar {
        to: u128,
        text: String,
        color: Color,
    },
    /// Fireworks and sparkles over the player: a jackpot.
    Fireworks(u128),
    /// A jackpot or a big win, to everyone and the Discord bridge.
    Announce(String),
    /// The player's casino menu shows something new.
    Show(u128),
    /// The player's casino menu opens again on its (new) view.
    Open(u128),
    /// The crash's boss bar: its line, its colour when it changes, its fill
    /// (0 to 100).
    Boss {
        text: String,
        color: Option<Color>,
        value: u32,
    },
    /// Who sees the boss bar (by name), when that changes.
    BossPlayers(Vec<String>),
    /// The boss bar goes away.
    BossHide,
}

/// What a player's casino menu shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Lobby,
    Roulette,
    /// Roulette's single numbers, to bet on one.
    RouletteNumbers,
    Slots,
    Crash,
    Blackjack,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Lobby => "Casino",
            View::Roulette | View::RouletteNumbers => "Casino · Roulette",
            View::Slots => "Casino · Slots",
            View::Crash => "Casino · Crash",
            View::Blackjack => "Casino · Blackjack",
        }
    }
}

/// A player's open casino menu.
#[derive(Clone, Debug)]
pub struct Seat {
    /// The menu's window, once open.
    pub window: Option<i32>,
    pub view: View,
    pub name: String,
    /// The stake the next bet takes.
    pub stake: i64,
    /// Roulette's table: the bet chosen and where the wheel stands.
    pub roulette: roulette::Table,
    /// The slot machine: where its reels stand.
    pub slots: slots::Machine,
    /// Crash: the automatic cash-out the next bet takes (hundredths, 0: none).
    pub crash_auto: u32,
    /// Blackjack: the last hand, shown until the next.
    pub blackjack: bj::Table,
    /// The frame last shown, so a menu is sent again only when it changed.
    pub shown: Option<i64>,
}

/// One player's day.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Day {
    pub wagered: i64,
    pub paid: i64,
    pub rounds: u32,
}

impl Day {
    pub fn lost(&self) -> i64 {
        (self.wagered - self.paid).max(0)
    }

    pub fn won(&self) -> i64 {
        (self.paid - self.wagered).max(0)
    }
}

/// A win of the day.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Win {
    pub name: String,
    pub game: String,
    pub stake: i64,
    pub paid: i64,
    pub at: i64,
}

/// A game's totals since the casino opened: the house edge as measured.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Totals {
    pub rounds: u64,
    pub wagered: i64,
    pub paid: i64,
}

/// `casino.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ledger {
    /// The day (days since 1970, UTC) `players` and `wins` are of.
    pub day: i64,
    pub players: BTreeMap<String, Day>,
    /// The day's biggest wins (by profit), biggest first.
    pub wins: Vec<Win>,
    pub totals: BTreeMap<String, Totals>,
}

/// The mark, in `economy.json`'s `ext`, that the pot got its float: kept with
/// the pot's money so one write saves both, and a lost or unreadable
/// `casino.json` can't make the float again.
const FLOATED: &str = "casino_float";

/// A bet placed and held, for a game to decide.
#[derive(Clone, Debug, PartialEq)]
pub struct Stake {
    pub uuid: u128,
    pub name: String,
    pub game: Game,
    pub amount: i64,
}

pub struct Casino {
    pub config: Config,
    /// `gambling.enabled`, the owner's switch for every game of chance.
    pub gambling: bool,
    pub ledger: Ledger,
    path: PathBuf,
    dirty: bool,
    pub seats: HashMap<u128, Seat>,
    pub roulette: Roulette,
    pub slots: Slots,
    pub crash: Crash,
    pub blackjack: Blackjack,
    /// The draws' random.
    pub rng: Rng,
    /// The boss bar as last sent: its frame, colour and players; None when
    /// hidden (and, at the start, a leftover of the last run to take away).
    boss: Option<(i64, Color, Vec<u128>)>,
    boss_reset: bool,
}

const USAGE: &str = "Usage: /casino [help|top|stats|roulette|slots|crash|blackjack]";

impl Casino {
    /// The casino of the world in `dir`; an unreadable `casino.json` is
    /// logged and left alone, the day starting again (the money is in
    /// `economy.json`, not here).
    pub fn open(dir: &Path, config: Config, gambling: bool) -> (Self, Option<String>) {
        let path = dir.join("casino.json");
        let (ledger, problem) = match read_json(&path) {
            Ok(l) => (l, None),
            Err(e) => (Ledger::default(), Some(e)),
        };
        let casino = Self {
            config,
            gambling,
            ledger,
            // An unreadable file is never written over.
            path: if problem.is_some() {
                dir.join("casino.json.new")
            } else {
                path
            },
            dirty: false,
            seats: HashMap::new(),
            roulette: Roulette::default(),
            slots: Slots::default(),
            crash: Crash::default(),
            blackjack: Blackjack::default(),
            rng: Rng::from_os(),
            boss: None,
            boss_reset: true,
        };
        (casino, problem)
    }

    pub fn save(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        write_json(&self.path, &self.ledger)?;
        self.dirty = false;
        Ok(())
    }

    /// Whether anything moves: the tick then comes back within a frame.
    pub fn animating(&self) -> bool {
        self.roulette.busy() || self.slots.busy() || self.crash.busy() || self.blackjack.busy()
    }

    /// Whether `uuid` has a round being decided, in any game.
    pub fn playing(&self, uuid: u128) -> bool {
        self.roulette.playing(uuid)
            || self.slots.playing(uuid)
            || self.crash.playing(uuid)
            || self.blackjack.playing(uuid)
    }

    fn today(&mut self, now: i64) {
        let day = now.div_euclid(DAY_MS);
        if self.ledger.day != day {
            self.ledger.day = day;
            self.ledger.players.clear();
            self.ledger.wins.clear();
            self.dirty = true;
        }
    }

    /// The player's day so far.
    pub fn day(&self, uuid: u128, now: i64) -> Day {
        if self.ledger.day != now.div_euclid(DAY_MS) {
            return Day::default();
        }
        self.ledger
            .players
            .get(&uuid_string(uuid))
            .cloned()
            .unwrap_or_default()
    }

    fn open_for_play(&self) -> Result<(), String> {
        if !self.gambling || !self.config.enabled {
            return Err("The casino is closed on this server.".into());
        }
        Ok(())
    }

    /// Takes a positive bet and holds its funds. Err: why not.
    pub fn bet(
        &mut self,
        economy: &mut Economy,
        uuid: u128,
        name: &str,
        game: Game,
        amount: i64,
        now: i64,
    ) -> Result<Stake, String> {
        self.open_for_play()?;
        if self.playing(uuid) {
            return Err("Your last round isn't over yet.".into());
        }
        economy.hold("casino_bet", uuid, amount)?;
        self.today(now);
        let day = self.ledger.players.entry(uuid_string(uuid)).or_default();
        day.wagered += amount;
        day.rounds += 1;
        self.dirty = true;
        Ok(Stake {
            uuid,
            name: name.to_owned(),
            game,
            amount,
        })
    }

    /// A round decided: the stake goes to the pot and `payout` (stake
    /// included; 0 for a loss) comes out of it, up to what it holds. The
    /// effects that tell the player, and everyone for a big win. `what`: how
    /// it ended ("17 Black").
    pub fn settle(
        &mut self,
        economy: &mut Economy,
        stake: &Stake,
        payout: i64,
        what: &str,
        now: i64,
    ) -> Vec<Fx> {
        let to = stake.uuid;
        let kind = format!("casino_{}", stake.game.id());
        let paid = match economy.settle_pool(&kind, to, stake.amount, payout, POT) {
            Ok(paid) => paid,
            Err(e) => {
                // Not decided: the stake goes back (or at the next start).
                self.void(economy, stake);
                return vec![Fx::Out(Out::Tell(
                    to,
                    format!("The round couldn't be paid ({e}): your bet was given back."),
                ))];
            }
        };
        self.today(now);
        self.ledger.players.entry(uuid_string(to)).or_default().paid += paid;
        let totals = self
            .ledger
            .totals
            .entry(stake.game.id().into())
            .or_default();
        totals.rounds += 1;
        totals.wagered += stake.amount;
        totals.paid += paid;
        self.dirty = true;
        let money = |n| economy.money(n);
        let mut fx = Vec::new();
        if paid < payout {
            fx.push(Fx::Out(Out::Tell(
                to,
                format!(
                    "The casino's pot held only {}: that is what you were paid, not {}.",
                    money(paid),
                    money(payout)
                ),
            )));
        }
        let profit = paid - stake.amount;
        if profit > 0 {
            let win = Win {
                name: stake.name.clone(),
                game: stake.game.title().into(),
                stake: stake.amount,
                paid,
                at: now,
            };
            let wins = &mut self.ledger.wins;
            let at = wins
                .iter()
                .position(|w| w.paid - w.stake < profit)
                .unwrap_or(wins.len());
            if at < WINS {
                wins.insert(at, win);
                wins.truncate(WINS);
            }
            let jackpot = paid >= stake.amount.saturating_mul(self.config.jackpot_multiple);
            fx.push(Fx::Title {
                to,
                title: format!("+{}", money(profit)),
                subtitle: what.to_owned(),
                color: if jackpot { Color::Gold } else { Color::Green },
            });
            fx.push(Fx::Sound {
                to,
                sound: "minecraft:entity.player.levelup",
                volume: 0.8,
                pitch: 1.0,
            });
            if jackpot {
                fx.push(Fx::Fireworks(to));
                fx.push(Fx::Sound {
                    to,
                    sound: "minecraft:ui.toast.challenge_complete",
                    volume: 1.0,
                    pitch: 1.0,
                });
            }
            if jackpot || profit >= self.config.announce_profit {
                fx.push(Fx::Announce(format!(
                    "{} won {} at {} ({}x their bet)!",
                    stake.name,
                    money(paid),
                    stake.game.title(),
                    paid / stake.amount.max(1)
                )));
            }
        } else if paid == stake.amount {
            fx.push(Fx::Bar {
                to,
                text: format!("{what}: your bet back"),
                color: Color::Gray,
            });
        } else {
            fx.push(Fx::Bar {
                to,
                text: format!("{what}: you lost {}", money(stake.amount - paid)),
                color: Color::Red,
            });
            fx.push(Fx::Sound {
                to,
                sound: "minecraft:block.anvil.land",
                volume: 0.25,
                pitch: 1.4,
            });
        }
        fx
    }

    /// Once, ever: a new casino's pot gets its float (what it lacks of
    /// it), made for it. A world whose pot already holds money (from before
    /// 0.30) gets only what it lacks.
    pub fn seed(&mut self, economy: &mut Economy) -> Option<String> {
        if economy.ext(FLOATED) == Some(&serde_json::Value::Bool(true)) {
            return None;
        }
        economy.set_ext(FLOATED, serde_json::Value::Bool(true));
        let lacks = self.config.float.max(0) - economy.pool(POT);
        if lacks <= 0 {
            return None;
        }
        economy
            .mint_pool("casino_float", POT, lacks, "the casino's float")
            .err()
    }

    /// The day's biggest wins, as lines.
    pub fn top_lines(&self, economy: &Economy, now: i64) -> Vec<String> {
        if self.ledger.day != now.div_euclid(DAY_MS) || self.ledger.wins.is_empty() {
            return vec!["No wins yet today.".into()];
        }
        self.ledger
            .wins
            .iter()
            .enumerate()
            .map(|(i, w)| {
                format!(
                    "{}. {}: {} at {} ({} bet)",
                    i + 1,
                    w.name,
                    economy.money(w.paid),
                    w.game,
                    economy.money(w.stake)
                )
            })
            .collect()
    }

    /// `/casino ...`: lines, and the view to open when it opens a menu.
    pub fn command(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        now: i64,
        line: &str,
    ) -> Option<(Vec<Out>, Option<View>)> {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.first() != Some(&"casino") {
            return None;
        }
        if let Err(e) = self.open_for_play() {
            return Some((vec![Out::Error(e)], None));
        }
        Some(match words[1..] {
            [] => (Vec::new(), Some(View::Lobby)),
            ["roulette"] => (Vec::new(), Some(View::Roulette)),
            ["slots"] => (Vec::new(), Some(View::Slots)),
            ["crash"] => (Vec::new(), Some(View::Crash)),
            ["blackjack"] => (Vec::new(), Some(View::Blackjack)),
            ["help"] => (
                self.help(economy).into_iter().map(Out::Reply).collect(),
                None,
            ),
            ["top"] => {
                let mut out = vec![Out::Reply("Today's biggest wins:".into())];
                out.extend(self.top_lines(economy, now).into_iter().map(Out::Reply));
                (out, None)
            }
            ["stats"] => {
                let d = self.day(me.uuid, now);
                let net = d.paid - d.wagered;
                (
                    vec![Out::Reply(format!(
                        "Today: {} round(s), {} bet, {} won back, net {}{}.",
                        d.rounds,
                        economy.money(d.wagered),
                        economy.money(d.paid),
                        if net > 0 { "+" } else { "" },
                        economy.money(net)
                    ))],
                    None,
                )
            }
            _ => (vec![Out::Error(USAGE.into())], None),
        })
    }

    /// `/casino help`: each game's odds and the house edge.
    pub fn help(&self, economy: &Economy) -> Vec<String> {
        let mut lines = vec![
            "Casino: positive bets with no daily win/loss limits or cooldown.".into(),
            "Every draw is made by the server (ChaCha20 seeded by the system), before the animation starts.".into(),
            format!(
                "Wins are paid from the casino's pot ({} now); a win larger than the pot is paid up to it. The house's profit stays in it.",
                economy.money(economy.pool(POT))
            ),
        ];
        lines.extend(roulette::HELP.iter().map(|s| s.to_string()));
        lines.extend(slots::HELP.iter().map(|s| s.to_string()));
        lines.extend(crash::HELP.iter().map(|s| s.to_string()));
        lines.extend(bj::HELP.iter().map(|s| s.to_string()));
        lines
    }

    /// The menu opens on `view` (from `/casino`).
    pub fn sit(&mut self, uuid: u128, name: &str, view: View) {
        let stake = 10;
        let seat = self.seats.entry(uuid).or_insert_with(|| Seat {
            window: None,
            view,
            name: name.to_owned(),
            stake,
            roulette: roulette::Table::default(),
            slots: slots::Machine::default(),
            crash_auto: 0,
            blackjack: bj::Table::default(),
            shown: None,
        });
        seat.view = view;
        seat.window = None;
        seat.shown = None;
    }

    /// The menu `window` closed: the seat goes; rounds still being decided
    /// carry on and are paid all the same.
    pub fn closed(&mut self, uuid: u128, window: i32, now: i64) -> bool {
        if self
            .seats
            .get(&uuid)
            .is_some_and(|s| s.window == Some(window))
        {
            self.seats.remove(&uuid);
            // A blackjack hand stands; the dealer plays it out.
            self.blackjack.stand(uuid, now);
            return true;
        }
        false
    }

    /// Whether `window` is `uuid`'s casino menu.
    pub fn owns(&self, uuid: u128, window: i32) -> bool {
        self.seats
            .get(&uuid)
            .is_some_and(|s| s.window == Some(window))
    }

    /// The cells of `uuid`'s menu.
    pub fn layout(&self, economy: &Economy, uuid: u128, now: i64) -> Vec<Option<Icon>> {
        let Some(seat) = self.seats.get(&uuid) else {
            return Vec::new();
        };
        let mut cells = vec![None; SIZE];
        match seat.view {
            View::Lobby => self.lobby(&mut cells, economy, uuid, now),
            View::Roulette | View::RouletteNumbers => {
                self.roulette.layout(&mut cells, uuid, seat, now);
                self.bar(&mut cells, economy, seat, uuid, now);
            }
            View::Slots => {
                self.slots.layout(&mut cells, uuid, seat, now);
                self.bar(&mut cells, economy, seat, uuid, now);
            }
            View::Crash => {
                self.crash
                    .layout(&mut cells, uuid, seat, now, self.config.crash_max);
                self.bar(&mut cells, economy, seat, uuid, now);
            }
            View::Blackjack => {
                self.blackjack.layout(&mut cells, uuid, seat);
                self.bar(&mut cells, economy, seat, uuid, now);
            }
        }
        cells
    }

    fn lobby(&self, cells: &mut [Option<Icon>], economy: &Economy, uuid: u128, now: i64) {
        for (i, c) in cells.iter_mut().enumerate() {
            if !(9..45).contains(&i) || i % 9 == 0 || i % 9 == 8 {
                *c = filler("minecraft:yellow_stained_glass_pane");
            }
        }
        cells[4] = icon("minecraft:gold_block", "The Casino", Color::Gold);
        for (slot, item, name, color) in LOBBY {
            cells[slot] = icon(item, name, color);
        }
        let day = self.day(uuid, now);
        cells[38] = icon(
            "minecraft:clock",
            format!(
                "Today: {} bet, {} won",
                economy.money(day.wagered),
                economy.money(day.paid)
            ),
            Color::Aqua,
        );
        cells[40] = icon(
            "minecraft:gold_ingot",
            format!("Your balance: {}", economy.money(economy.balance(uuid))),
            Color::Gold,
        );
        cells[42] = icon(
            "minecraft:emerald",
            format!("The casino's pot: {}", economy.money(economy.pool(POT))),
            Color::Green,
        );
        // The day's biggest wins, across the top of the bottom bar.
        let wins = if self.ledger.day == now.div_euclid(DAY_MS) {
            &self.ledger.wins[..]
        } else {
            &[]
        };
        for (i, w) in wins.iter().take(5).enumerate() {
            let item = [
                "minecraft:nether_star",
                "minecraft:diamond",
                "minecraft:emerald",
                "minecraft:gold_ingot",
                "minecraft:gold_nugget",
            ][i];
            cells[47 + i] = icon(
                item,
                format!(
                    "Today #{}: {} won {} at {}",
                    i + 1,
                    w.name,
                    economy.money(w.paid),
                    w.game
                ),
                Color::Gold,
            );
        }
        if wins.is_empty() {
            cells[49] = icon("minecraft:nether_star", "No wins yet today", Color::Gray);
        }
    }

    /// The bottom row every game shares: back, help, the stake, the balance,
    /// the day; the last cell is the game's own.
    fn bar(
        &self,
        cells: &mut [Option<Icon>],
        economy: &Economy,
        seat: &Seat,
        uuid: u128,
        _now: i64,
    ) {
        for c in &mut cells[45..53] {
            *c = filler("minecraft:black_stained_glass_pane");
        }
        cells[45] = icon("minecraft:arrow", "Back to the lobby", Color::White);
        cells[46] = icon(
            "minecraft:book",
            "Odds and rules (/casino help)",
            Color::White,
        );
        cells[47] = icon(
            "minecraft:red_stained_glass_pane",
            "Less (shift: the least)",
            Color::Red,
        );
        cells[48] = icon(
            "minecraft:gold_ingot",
            format!("Bet: {}", economy.money(seat.stake)),
            Color::Gold,
        );
        cells[49] = icon(
            "minecraft:lime_stained_glass_pane",
            "More (shift: the most)",
            Color::Green,
        );
        cells[50] = icon(
            "minecraft:sunflower",
            format!("Balance: {}", economy.money(economy.balance(uuid))),
            Color::Gold,
        );
        cells[51] = icon("minecraft:clock", "No daily betting limits", Color::Aqua);
    }

    /// A click on cell `slot` of `uuid`'s menu (shift: a shift-click).
    pub fn click(
        &mut self,
        economy: &mut Economy,
        uuid: u128,
        slot: usize,
        shift: bool,
        now: i64,
    ) -> Vec<Fx> {
        let Some(seat) = self.seats.get_mut(&uuid) else {
            return Vec::new();
        };
        let view = seat.view;
        let in_game = view != View::Lobby;
        if in_game && (45..=51).contains(&slot) {
            let (min, max) = (1, super::smp_economy::MAX_AMOUNT);
            match slot {
                45 => seat.view = View::Lobby,
                46 => {
                    let lines = self.help(economy);
                    return lines
                        .into_iter()
                        .map(|l| Fx::Out(Out::Tell(uuid, l)))
                        .collect();
                }
                47 => {
                    seat.stake = if shift {
                        min
                    } else {
                        step(seat.stake, false).clamp(min, max)
                    }
                }
                49 => {
                    seat.stake = if shift {
                        max
                    } else {
                        step(seat.stake, true).clamp(min, max)
                    }
                }
                _ => return Vec::new(),
            }
            let click = Fx::Sound {
                to: uuid,
                sound: "minecraft:ui.button.click",
                volume: 0.4,
                pitch: 1.2,
            };
            return if slot == 45 {
                vec![click, Fx::Open(uuid)]
            } else {
                vec![click, Fx::Show(uuid)]
            };
        }
        match view {
            View::Lobby => {
                let to = match slot {
                    19 => View::Roulette,
                    21 => View::Slots,
                    23 => View::Crash,
                    25 => View::Blackjack,
                    31 => {
                        let lines = self.help(economy);
                        return lines
                            .into_iter()
                            .map(|l| Fx::Out(Out::Tell(uuid, l)))
                            .collect();
                    }
                    _ => return Vec::new(),
                };
                seat.view = to;
                vec![Fx::Open(uuid)]
            }
            View::Roulette | View::RouletteNumbers => {
                // The table waits while its wheel turns.
                if self.roulette.playing(uuid) {
                    return Vec::new();
                }
                let Some(seat) = self.seats.get_mut(&uuid) else {
                    return Vec::new();
                };
                match roulette::click(seat, slot) {
                    roulette::Click::Nothing => Vec::new(),
                    roulette::Click::Show => vec![Fx::Show(uuid)],
                    roulette::Click::Spin => self.spin(economy, uuid, now),
                }
            }
            View::Crash => self.crash_click(economy, uuid, slot, now),
            View::Blackjack => self.blackjack_click(economy, uuid, slot, now),
            View::Slots => {
                if slot != 53 || self.slots.playing(uuid) {
                    return Vec::new();
                }
                let (name, amount) = (seat.name.clone(), seat.stake);
                match self.bet(economy, uuid, &name, Game::Slots, amount, now) {
                    Err(e) => vec![Fx::Bar {
                        to: uuid,
                        text: e,
                        color: Color::Red,
                    }],
                    Ok(stake) => {
                        self.slots.pull(stake, now, &mut self.rng);
                        if let Some(seat) = self.seats.get_mut(&uuid) {
                            seat.shown = Some(0);
                        }
                        vec![
                            Fx::Show(uuid),
                            Fx::Sound {
                                to: uuid,
                                sound: "minecraft:block.lever.click",
                                volume: 0.8,
                                pitch: 0.8,
                            },
                        ]
                    }
                }
            }
        }
    }

    fn crash_click(&mut self, economy: &mut Economy, uuid: u128, slot: usize, now: i64) -> Vec<Fx> {
        let Some(seat) = self.seats.get_mut(&uuid) else {
            return Vec::new();
        };
        if slot == 42 {
            let i = crash::AUTOS
                .iter()
                .position(|a| *a == seat.crash_auto)
                .unwrap_or(0);
            seat.crash_auto = crash::AUTOS[(i + 1) % crash::AUTOS.len()];
            return vec![Fx::Show(uuid)];
        }
        if slot != 53 {
            return Vec::new();
        }
        if self.crash.playing(uuid) {
            return match self.crash.cash_out(uuid, now) {
                Some((bet, h)) => self.crash_paid(economy, &bet, h, now),
                None => Vec::new(),
            };
        }
        if !self.crash.open() {
            return vec![Fx::Bar {
                to: uuid,
                text: "Wait for the next round.".into(),
                color: Color::Yellow,
            }];
        }
        let (name, amount, auto) = (seat.name.clone(), seat.stake, seat.crash_auto);
        match self.bet(economy, uuid, &name, Game::Crash, amount, now) {
            Err(e) => vec![Fx::Bar {
                to: uuid,
                text: e,
                color: Color::Red,
            }],
            Ok(stake) => {
                self.crash.join(stake, auto, now);
                vec![
                    Fx::Show(uuid),
                    Fx::Sound {
                        to: uuid,
                        sound: "minecraft:block.note_block.pling",
                        volume: 0.6,
                        pitch: 1.2,
                    },
                ]
            }
        }
    }

    fn blackjack_click(
        &mut self,
        economy: &mut Economy,
        uuid: u128,
        slot: usize,
        now: i64,
    ) -> Vec<Fx> {
        let card = Fx::Sound {
            to: uuid,
            sound: "minecraft:item.book.page_turn",
            volume: 0.8,
            pitch: 1.0,
        };
        let done = match slot {
            53 => {
                if self.blackjack.playing(uuid) {
                    return Vec::new();
                }
                let Some(seat) = self.seats.get(&uuid) else {
                    return Vec::new();
                };
                let (name, amount) = (seat.name.clone(), seat.stake);
                match self.bet(economy, uuid, &name, Game::Blackjack, amount, now) {
                    Err(e) => {
                        return vec![Fx::Bar {
                            to: uuid,
                            text: e,
                            color: Color::Red,
                        }];
                    }
                    Ok(stake) => self.blackjack.deal(stake, now, &mut self.rng),
                }
            }
            38 => self.blackjack.hit(uuid, now, &mut self.rng),
            40 => {
                self.blackjack.stand(uuid, now);
                None
            }
            42 => {
                if !self.blackjack.may_double(uuid) {
                    return Vec::new();
                }
                let extra = self
                    .blackjack
                    .hands
                    .iter()
                    .find(|h| h.stake.uuid == uuid)
                    .map_or(0, |h| h.stake.amount);
                if let Err(e) = self.raise(economy, uuid, extra, now) {
                    return vec![Fx::Bar {
                        to: uuid,
                        text: e,
                        color: Color::Red,
                    }];
                }
                self.blackjack.doubled(uuid, extra, now, &mut self.rng)
            }
            _ => return Vec::new(),
        };
        let mut fx = vec![card, Fx::Show(uuid)];
        if let Some(d) = done {
            fx.extend(self.blackjack_paid(economy, d, now));
        }
        fx
    }

    /// A bet already in play raised by `extra` (a double), held the same way.
    fn raise(
        &mut self,
        economy: &mut Economy,
        uuid: u128,
        extra: i64,
        now: i64,
    ) -> Result<(), String> {
        economy.hold("casino_bet", uuid, extra)?;
        self.today(now);
        self.ledger
            .players
            .entry(uuid_string(uuid))
            .or_default()
            .wagered += extra;
        self.dirty = true;
        Ok(())
    }

    fn blackjack_paid(&mut self, economy: &mut Economy, d: bj::Done, now: i64) -> Vec<Fx> {
        let uuid = d.hand.stake.uuid;
        let payout = d.end.payout(d.hand.stake.amount);
        let what = format!(
            "{}: you {}, the dealer {}",
            bj::end_line(d.end),
            bj::total(&d.hand.player).0,
            bj::total(&d.hand.dealer).0
        );
        if let Some(seat) = self.seats.get_mut(&uuid) {
            seat.blackjack.last = Some((d.hand.player.clone(), d.hand.dealer.clone(), d.end));
        }
        let mut fx = self.settle(economy, &d.hand.stake, payout, &what, now);
        fx.push(Fx::Show(uuid));
        fx
    }

    /// A crash bet cashed out at `h` hundredths.
    fn crash_paid(&mut self, economy: &mut Economy, bet: &crash::Bet, h: u32, now: i64) -> Vec<Fx> {
        let payout = bet.stake.amount.saturating_mul(i64::from(h)) / 100;
        let what = format!("Crash: cashed out at {}", crash::times(h));
        let mut fx = self.settle(economy, &bet.stake, payout, &what, now);
        fx.push(Fx::Show(bet.stake.uuid));
        fx
    }

    /// A held bet given back, the round not played (its player left first).
    fn void(&mut self, economy: &mut Economy, stake: &Stake) {
        let _ = economy.release("casino_void", stake.uuid, stake.amount);
        if let Some(day) = self.ledger.players.get_mut(&uuid_string(stake.uuid)) {
            day.wagered -= stake.amount;
            day.rounds = day.rounds.saturating_sub(1);
        }
        self.dirty = true;
    }

    /// The players on (once a second): a crash bet of one who left is
    /// cashed out now (in flight) or given back (before it).
    pub fn gone(&mut self, economy: &mut Economy, online: &[u128], now: i64) -> Vec<Fx> {
        let away: Vec<u128> = self
            .crash
            .bets
            .iter()
            .map(|b| b.stake.uuid)
            .filter(|u| !online.contains(u))
            .collect();
        let mut fx = Vec::new();
        for uuid in away {
            if let Some((bet, h)) = self.crash.cash_out(uuid, now) {
                fx.extend(self.crash_paid(economy, &bet, h, now));
            } else if let Some(bet) = self.crash.withdraw(uuid) {
                self.void(economy, &bet.stake);
            }
        }
        let away: Vec<u128> = self
            .blackjack
            .hands
            .iter()
            .map(|h| h.stake.uuid)
            .filter(|u| !online.contains(u))
            .collect();
        for uuid in away {
            self.blackjack.stand(uuid, now);
        }
        self.seats.retain(|u, _| online.contains(u));
        fx
    }

    /// The crash round's step: take-off, automatic cash-outs, the crash, its
    /// pictures and the boss bar.
    fn crash_tick(&mut self, economy: &mut Economy, now: i64) -> Vec<Fx> {
        let mut fx = Vec::new();
        if self.boss_reset {
            self.boss_reset = false;
            fx.push(Fx::BossHide);
        }
        if !self.crash.busy() && self.boss.is_none() {
            return fx;
        }
        let (edge, max) = (self.config.crash_edge_percent, self.config.crash_max);
        let step = self.crash.step(now, &mut self.rng, edge, max);
        let watching = |seats: &HashMap<u128, Seat>| -> Vec<u128> {
            seats
                .iter()
                .filter(|(_, s)| s.view == View::Crash && s.window.is_some())
                .map(|(u, _)| *u)
                .collect()
        };
        if step.took_off {
            for u in self
                .crash
                .bets
                .iter()
                .map(|b| b.stake.uuid)
                .chain(watching(&self.seats))
                .collect::<Vec<_>>()
            {
                fx.push(Fx::Sound {
                    to: u,
                    sound: "minecraft:entity.firework_rocket.launch",
                    volume: 0.8,
                    pitch: 1.0,
                });
            }
        }
        for (bet, h) in step.cashed {
            fx.extend(self.crash_paid(economy, &bet, h, now));
        }
        if let Some((h, lost)) = step.crashed {
            for bet in lost {
                let what = format!("Crashed at {}", crash::times(h));
                fx.push(Fx::Title {
                    to: bet.stake.uuid,
                    title: "CRASHED".into(),
                    subtitle: what.clone(),
                    color: Color::Red,
                });
                fx.extend(self.settle(economy, &bet.stake, 0, &what, now));
            }
            for u in watching(&self.seats) {
                fx.push(Fx::Sound {
                    to: u,
                    sound: "minecraft:entity.generic.explode",
                    volume: 0.5,
                    pitch: 1.2,
                });
            }
        }
        // The pictures: the menus watching, the boss bar, the bettors' action bar.
        let frame = self.crash.frame(now);
        if self.boss.as_ref().map(|b| b.0) == Some(frame) {
            return fx;
        }
        for (u, seat) in &mut self.seats {
            if seat.view == View::Crash && seat.shown != Some(frame) {
                seat.shown = Some(frame);
                fx.push(Fx::Show(*u));
            }
        }
        match self.crash.boss(now, max) {
            None => {
                self.boss = None;
                fx.push(Fx::BossHide);
            }
            Some((text, color, value)) => {
                let mut to: Vec<u128> = self.crash.bets.iter().map(|b| b.stake.uuid).collect();
                to.extend(watching(&self.seats));
                to.sort_unstable();
                to.dedup();
                let (last_color, last_to) = match &self.boss {
                    Some((_, c, t)) => (Some(*c), Some(t.clone())),
                    None => (None, None),
                };
                if last_to.as_ref() != Some(&to) {
                    let names = to
                        .iter()
                        .filter_map(|u| {
                            self.seats.get(u).map(|s| s.name.clone()).or_else(|| {
                                self.crash
                                    .bets
                                    .iter()
                                    .find(|b| b.stake.uuid == *u)
                                    .map(|b| b.stake.name.clone())
                            })
                        })
                        .collect();
                    fx.push(Fx::BossPlayers(names));
                }
                if let Some(h) = self.crash.now_multiple(now) {
                    for b in &self.crash.bets {
                        fx.push(Fx::Bar {
                            to: b.stake.uuid,
                            text: format!(
                                "Crash {}: cash out for {} (/casino crash)",
                                crash::times(h),
                                b.stake.amount * i64::from(h) / 100
                            ),
                            color: Color::Green,
                        });
                    }
                }
                fx.push(Fx::Boss {
                    text,
                    color: (last_color != Some(color)).then_some(color),
                    value,
                });
                self.boss = Some((frame, color, to));
            }
        }
        fx
    }

    fn spin(&mut self, economy: &mut Economy, uuid: u128, now: i64) -> Vec<Fx> {
        let Some(seat) = self.seats.get(&uuid) else {
            return Vec::new();
        };
        let Some(bet) = seat.roulette.bet else {
            return vec![Fx::Bar {
                to: uuid,
                text: "Pick a bet first, then spin.".into(),
                color: Color::Yellow,
            }];
        };
        let (name, amount, from) = (seat.name.clone(), seat.stake, seat.roulette.at);
        match self.bet(economy, uuid, &name, Game::Roulette, amount, now) {
            Err(e) => vec![Fx::Bar {
                to: uuid,
                text: e,
                color: Color::Red,
            }],
            Ok(stake) => {
                self.roulette.spin(stake, bet, from, now, &mut self.rng);
                if let Some(seat) = self.seats.get_mut(&uuid) {
                    seat.shown = Some(0);
                }
                vec![Fx::Show(uuid)]
            }
        }
    }

    /// Each tick while something moves (`animating`), and once a second:
    /// the frames due, the rounds that end, the pot's float (once).
    pub fn tick(&mut self, economy: &mut Economy, now: i64) -> Vec<Fx> {
        let mut fx = Vec::new();
        for (spin, n) in self.roulette.ended(now) {
            let payout = spin.bet.payout(n, spin.stake.amount);
            if let Some(seat) = self.seats.get_mut(&spin.stake.uuid) {
                seat.roulette.at = spin.stop;
                seat.roulette.last = Some((n, payout > 0));
                seat.shown = None;
            }
            let what = format!("{} · {}", roulette::pocket_name(n), spin.bet.label());
            fx.push(Fx::Sound {
                to: spin.stake.uuid,
                sound: "minecraft:block.note_block.bell",
                volume: 0.8,
                pitch: 1.0,
            });
            fx.extend(self.settle(economy, &spin.stake, payout, &what, now));
            fx.push(Fx::Show(spin.stake.uuid));
        }
        for pull in self.slots.ended(now) {
            let payout = pull.stake.amount * slots::multiple(slots::line(pull.stops));
            if let Some(seat) = self.seats.get_mut(&pull.stake.uuid) {
                seat.slots.stops = pull.stops;
                seat.slots.last = Some(payout > 0);
                seat.shown = None;
            }
            let what = slots::line_name(pull.stops);
            fx.extend(self.settle(economy, &pull.stake, payout, &what, now));
            fx.push(Fx::Show(pull.stake.uuid));
        }
        fx.extend(self.crash_tick(economy, now));
        let (done, changed) = self.blackjack.step(now, &mut self.rng);
        for uuid in changed {
            fx.push(Fx::Show(uuid));
            fx.push(Fx::Sound {
                to: uuid,
                sound: "minecraft:item.book.page_turn",
                volume: 0.8,
                pitch: 1.0,
            });
        }
        for d in done {
            fx.extend(self.blackjack_paid(economy, d, now));
        }
        // A new picture for each menu watching its round, 5 a second at most.
        for (uuid, seat) in &mut self.seats {
            let (frame, sound, pitch) = match seat.view {
                View::Roulette => match self.roulette.frame(*uuid, now) {
                    Some(f) => (f, "minecraft:block.note_block.hat", 0.8 + f as f32 * 0.03),
                    None => continue,
                },
                View::Slots => match self.slots.frame(*uuid, now) {
                    Some((f, true)) => (f, "minecraft:block.note_block.basedrum", 1.0),
                    Some((f, false)) => (f, "minecraft:block.note_block.hat", 1.4),
                    None => continue,
                },
                _ => continue,
            };
            if seat.shown != Some(frame) {
                seat.shown = Some(frame);
                fx.push(Fx::Show(*uuid));
                fx.push(Fx::Sound {
                    to: *uuid,
                    sound,
                    volume: 0.5,
                    pitch,
                });
            }
        }
        if let Some(e) = self.seed(economy) {
            fx.push(Fx::Out(Out::All(format!(
                "[casino] the pot's float failed: {e}"
            ))));
        }
        fx
    }
}

/// The lobby's games and lines: slot, item, name, colour.
const LOBBY: [(usize, &str, &str, Color); 7] = [
    (
        19,
        "minecraft:red_concrete",
        "Roulette: European, one zero (house edge 2.7%)",
        Color::Red,
    ),
    (
        21,
        "minecraft:nether_star",
        "Slots: 3 reels, top prize 500x (94.2% paid back)",
        Color::Aqua,
    ),
    (
        23,
        "minecraft:firework_rocket",
        "Crash: cash out before it crashes (96% paid back)",
        Color::Green,
    ),
    (
        25,
        "minecraft:paper",
        "Blackjack: dealer stands on 17, blackjack pays 3 to 2",
        Color::White,
    ),
    (
        29,
        "minecraft:sunflower",
        "Coinflip, player against player: /coinflip <amount>",
        Color::Yellow,
    ),
    (
        31,
        "minecraft:book",
        "Odds and rules (/casino help)",
        Color::White,
    ),
    (33, "minecraft:name_tag", "Lottery: /lottery", Color::Yellow),
];

/// The stakes `-`/`+` step through: 1, 2, 5, 10, 25, 50, 100, 250...
fn step(stake: i64, up: bool) -> i64 {
    let mut steps = Vec::new();
    let mut base = 1i64;
    while base <= super::smp_economy::MAX_AMOUNT {
        steps.extend([base, base * 25 / 10, base * 5]);
        base *= 10;
    }
    if up {
        steps.into_iter().find(|s| *s > stake).unwrap_or(stake)
    } else {
        steps
            .into_iter()
            .rev()
            .find(|s| *s < stake)
            .unwrap_or(stake)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::smp_economy::Config as EconomyConfig;

    pub const A: u128 = 1;
    pub const B: u128 = 2;

    /// An economy where A and B hold 10,000 each and the pot `pot`.
    pub fn setup(name: &str, pot: i64) -> (Economy, Casino, PathBuf) {
        let dir = std::env::temp_dir().join(format!("smp-casino-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Economy::open(&dir, EconomyConfig::default()).unwrap();
        e.seen(A, "Alex");
        e.seen(B, "Steve");
        e.adjust("eco_give", A, 10_000, "test").unwrap();
        e.adjust("eco_give", B, 10_000, "test").unwrap();
        if pot > 0 {
            e.seen(99, "Bank");
            e.adjust("eco_give", 99, pot, "test").unwrap();
            e.to_pool("test", 99, POT, pot, "test").unwrap();
        }
        let (mut c, problem) = Casino::open(&dir, Config::default(), true);
        assert!(problem.is_none());
        // Draws the same every run; the pot as set up.
        c.rng = Rng::from_seed([7; 32]);
        e.set_ext(FLOATED, serde_json::Value::Bool(true));
        (e, c, dir)
    }

    pub fn money(e: &Economy) -> i64 {
        e.balance(A) + e.balance(B) + e.balance(99) + e.held(A) + e.held(B) + e.pool(POT)
    }

    #[test]
    fn chacha20_matches_rfc_8439() {
        // RFC 8439, 2.3.2: key 00..1f, counter 1, nonce 00:00:00:09 00:00:00:4a 00:00:00:00.
        let mut seed = [0u8; 32];
        for (i, b) in seed.iter_mut().enumerate() {
            *b = i as u8;
        }
        let rng = Rng::from_seed(seed);
        let out = chacha20(&rng.key, [1, 0x0900_0000, 0x4a00_0000, 0]);
        assert_eq!(
            out,
            [
                0xe4e7f110, 0x15593bd1, 0x1fdd0f50, 0xc47120a3, 0xc7f4d1c7, 0x0368c033, 0x9aaa2204,
                0x4e6cd4c3, 0x466482d2, 0x09aa9f07, 0x05d7c214, 0xa2028bd9, 0xd19c12b5, 0xb94e16de,
                0xe883d0cb, 0x4e3c50a2,
            ]
        );
    }

    #[test]
    fn the_rng_is_uniform_and_keyed_by_the_system() {
        let mut a = Rng::from_os();
        let mut b = Rng::from_os();
        assert_ne!(a.next_u64(), b.next_u64(), "two seeds");
        let mut seeded = Rng::from_seed([1; 32]);
        let mut counts = [0u32; 37];
        for _ in 0..37_000 {
            counts[seeded.below(37) as usize] += 1;
        }
        // Each pocket ~1000; a fair draw stays well inside 800..1200.
        assert!(counts.iter().all(|c| (800..1200).contains(c)), "{counts:?}");
    }

    /// The server's cost while 20 players play at once (8 at slots, 6 at
    /// roulette, 6 in one crash round), each starting a new round as soon as
    /// the last ends: the casino's step every tick (50 ms) for 60 s, with
    /// the menu's cells built for each picture shown. Prints the time a
    /// tick. `cargo test --release -- --ignored --nocapture twenty_players`.
    #[test]
    #[ignore]
    fn twenty_players_playing_cost_per_tick() {
        let (mut e, mut c, dir) = setup("cost", 1_000_000);
        c.config.cooldown_ms = 0;
        c.config.daily_loss_cap = i64::MAX / 4;
        c.config.daily_win_cap = i64::MAX / 4;
        let players: Vec<u128> = (100..120).collect();
        for (i, u) in players.iter().enumerate() {
            e.seen(*u, &format!("P{i}"));
            e.adjust("eco_give", *u, 1_000_000_000, "test").unwrap();
            let view = match i {
                0..8 => View::Slots,
                8..14 => View::Roulette,
                _ => View::Crash,
            };
            c.sit(*u, &format!("P{i}"), view);
            c.seats.get_mut(u).unwrap().window = Some(i as i32 + 1);
            if view == View::Roulette {
                c.click(&mut e, *u, 18, false, 0);
            }
        }
        let (mut total, mut worst, mut shows, mut ticks) = (
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            0u64,
            0u64,
        );
        let mut now = 0;
        while now < 60_000 {
            let t = std::time::Instant::now();
            for u in &players {
                if !c.playing(*u) {
                    let fx = c.click(&mut e, *u, 53, false, now);
                    shows += fx.iter().filter(|f| matches!(f, Fx::Show(_))).count() as u64;
                }
            }
            let fx = c.tick(&mut e, now);
            for f in &fx {
                if let Fx::Show(u) = f {
                    std::hint::black_box(c.layout(&e, *u, now));
                    shows += 1;
                }
            }
            let took = t.elapsed();
            total += took;
            worst = worst.max(took);
            ticks += 1;
            now += 50;
        }
        let money_now = e.balance(A) + e.balance(B);
        assert!(money_now > 0);
        println!(
            "20 players playing: {:.1} µs a tick on average, {:.1} µs at worst, {:.1} pictures a second for 20 menus",
            total.as_secs_f64() * 1e6 / ticks as f64,
            worst.as_secs_f64() * 1e6,
            shows as f64 / 60.0
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stake_steps() {
        assert_eq!(step(10, true), 25);
        assert_eq!(step(25, true), 50);
        assert_eq!(step(50, true), 100);
        assert_eq!(step(100, false), 50);
        assert_eq!(step(30, false), 25);
    }

    #[test]
    fn legacy_bet_limits_and_cooldown_do_not_block_play() {
        let (mut e, mut c, dir) = setup("limits", 50_000);
        c.config.min_bet = 10;
        c.config.max_bet = 1000;
        c.config.daily_loss_cap = 1;
        c.config.cooldown_ms = 2000;
        for amount in [5, 1001, 2000] {
            let stake = c.bet(&mut e, A, "Alex", Game::Roulette, amount, 0).unwrap();
            c.roulette.rounds.push(roulette::Spin::test(stake.clone()));
            assert!(
                c.bet(&mut e, A, "Alex", Game::Roulette, 1, 0)
                    .unwrap_err()
                    .contains("isn't over")
            );
            c.roulette.rounds.clear();
            c.settle(&mut e, &stake, 0, "lost", 0);
        }
        assert_eq!(e.held(A), 0);
        assert!(c.bet(&mut e, A, "Alex", Game::Roulette, 0, 0).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wins_and_raises_ignore_legacy_daily_caps() {
        let (mut e, mut c, dir) = setup("wincap", 50_000);
        c.config.daily_win_cap = 1;
        c.config.cooldown_ms = 2000;
        let s = c.bet(&mut e, A, "Alex", Game::Roulette, 700, 0).unwrap();
        c.settle(&mut e, &s, 1400, "won", 0);
        assert_eq!(e.balance(A), 10_700);
        let s = c.bet(&mut e, A, "Alex", Game::Slots, 1, 0).unwrap();
        c.settle(&mut e, &s, 0, "lost", 0);
        assert!(c.raise(&mut e, A, 1001, 0).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wins_come_from_the_pot_and_are_capped_by_it() {
        let (mut e, mut c, dir) = setup("pot", 1000);
        let before = money(&e);
        let s = c.bet(&mut e, A, "Alex", Game::Roulette, 100, 0).unwrap();
        // Lost: the stake fills the pot.
        c.settle(&mut e, &s, 0, "lost", 0);
        assert_eq!((e.balance(A), e.pool(POT)), (9900, 1100));
        // A 35-to-1 win: 3,600 asked, the pot holds 1,100 + the 100 staked.
        let s = c
            .bet(&mut e, A, "Alex", Game::Roulette, 100, 10_000)
            .unwrap();
        let fx = c.settle(&mut e, &s, 3600, "17 Black", 10_000);
        assert_eq!((e.balance(A), e.pool(POT)), (9900 - 100 + 1200, 0));
        assert!(
            fx.iter().any(
                |f| matches!(f, Fx::Out(Out::Tell(A, s)) if s.contains("held only 1,200 coins"))
            ),
            "{fx:?}"
        );
        assert_eq!(money(&e), before, "no coin made or lost");
        assert_eq!(c.ledger.wins[0].paid, 1200);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_jackpot_is_announced_with_fireworks() {
        let (mut e, mut c, dir) = setup("jackpot", 100_000);
        let s = c.bet(&mut e, B, "Steve", Game::Roulette, 100, 0).unwrap();
        let fx = c.settle(&mut e, &s, 5000, "jackpot", 0);
        assert!(fx.contains(&Fx::Fireworks(B)));
        assert!(fx.iter().any(|f| matches!(f, Fx::Announce(s) if s.contains("Steve won 5,000 coins at Roulette (50x"))), "{fx:?}");
        let small = c
            .bet(&mut e, B, "Steve", Game::Roulette, 100, 10_000)
            .unwrap();
        let fx = c.settle(&mut e, &small, 200, "Red", 10_000);
        assert!(
            !fx.iter()
                .any(|f| matches!(f, Fx::Announce(_) | Fx::Fireworks(_)))
        );
        assert_eq!(
            c.top_lines(&e, 10_000)[0],
            "1. Steve: 5,000 coins at Roulette (100 coins bet)"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_new_pot_gets_its_float_once_and_keeps_the_house_edge() {
        let (mut e, mut c, dir) = setup("seed", 0);
        e.set_ext(FLOATED, serde_json::Value::Bool(false));
        assert_eq!(c.seed(&mut e), None);
        assert_eq!(e.pool(POT), 20_000);
        assert_eq!(e.flows().minted["casino_float"], 20_000);
        // Never again, however low the pot runs.
        e.pool_move("test", POT, "elsewhere", 19_000, "test")
            .unwrap();
        assert_eq!(c.seed(&mut e), None);
        assert_eq!(e.pool(POT), 1000);
        // Nor after a restart that lost casino.json: the mark is the economy's.
        e.save().unwrap();
        let mut e = Economy::open(&dir, EconomyConfig::default()).unwrap();
        std::fs::remove_file(dir.join("casino.json")).ok();
        let (mut c, _) = Casino::open(&dir, Config::default(), true);
        assert_eq!(c.seed(&mut e), None);
        assert_eq!(e.pool(POT), 1000, "no second float");
        // An old pot with money gets only what it lacks.
        let (mut e, mut c, _) = setup("seed-old", 25_000);
        e.set_ext(FLOATED, serde_json::Value::Bool(false));
        c.seed(&mut e);
        assert_eq!(e.pool(POT), 25_000, "above the float: nothing made");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn closed_off_and_the_ledger_survives_a_restart() {
        let (mut e, mut c, dir) = setup("off", 1000);
        c.gambling = false;
        assert!(
            c.bet(&mut e, A, "Alex", Game::Roulette, 100, 0)
                .unwrap_err()
                .contains("closed")
        );
        c.gambling = true;
        let s = c.bet(&mut e, A, "Alex", Game::Roulette, 100, 0).unwrap();
        c.settle(&mut e, &s, 200, "Red", 0);
        c.save().unwrap();
        let (again, _) = Casino::open(&dir, Config::default(), true);
        assert_eq!(
            again.day(A, 0),
            Day {
                wagered: 100,
                paid: 200,
                rounds: 1
            }
        );
        // A crash while a bet is held gives it back.
        c.bet(&mut e, B, "Steve", Game::Roulette, 100, 0).unwrap();
        e.save().unwrap();
        let e2 = Economy::open(&dir, EconomyConfig::default()).unwrap();
        assert_eq!((e2.balance(B), e2.held(B)), (10_000, 0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
