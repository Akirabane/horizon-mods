// The server market: the server buys and sells vanilla items at the prices
// of `config.json`'s `market`, the economy's way to earn without changing
// survival. `/worth`, `/sell hand|all|<item> [count]`, `/buy <item> [count]`,
// and the `/shop` menu (smp_shop) on the same prices. Only plain items (no
// damage, no name or enchantment) are traded, and only by players in survival
// or adventure mode. Items move first, then money, both logged; a sale the
// log refuses gives the items back. Nobody gets rich from it: buying always
// costs more than selling (an item priced otherwise isn't bought), no chain
// of recipes turns bought items into dearer ones (smp_recipes; such an item
// isn't bought either). Sales have no daily caps (`market.json` records
// the day's sales). No game types: the glue
// lends the player's items as a `Bag`. Added to the game as
// `game::smp_market`; also a module of this crate, for its tests.

use super::smp_economy::{Economy, MAX_AMOUNT, Out, Player, parse_amount};
use super::smp_market_prices::{
    Book, Dynamic, MINUTE_MS, Point, Quote, Span, arrow, base_mid, one_price, to_get, to_pay,
};
use super::smp_store::{read_json, uuid_string, write_json};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// A UTC day, in milliseconds.
const DAY_MS: i64 = 86_400_000;

/// What the server pays for one item (`sell`) and asks for one (`buy`); 0
/// closes that side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Price {
    pub buy: i64,
    pub sell: i64,
}

/// `config.json`'s `market`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    /// By item id, `minecraft:diamond`. Items that stack to 64.
    pub items: BTreeMap<String, Price>,
    /// Legacy setting, ignored: selling has no daily coin cap.
    pub daily_sell_coins: i64,
    /// Legacy setting, ignored: selling has no daily item cap.
    pub daily_sell_items: u32,
    /// Prices that move with supply and demand (smp_market_prices); the
    /// item prices above are then the base ones.
    pub dynamic: Dynamic,
}

/// The default prices: (item, buy, sell). Farm, mine and mob produce the
/// market buys cheaply; blocks, food and tools it only sells.
pub const PRICES: &[(&str, i64, i64)] = &[
    // Farming.
    ("wheat", 3, 1),
    ("wheat_seeds", 2, 0),
    ("carrot", 3, 1),
    ("potato", 3, 1),
    ("beetroot", 3, 1),
    ("sugar_cane", 3, 1),
    ("pumpkin", 8, 3),
    ("melon_slice", 2, 1),
    ("cactus", 3, 1),
    ("bone_meal", 3, 0),
    // Mining.
    ("coal", 8, 2),
    ("copper_ingot", 6, 2),
    ("iron_ingot", 30, 8),
    ("gold_ingot", 40, 10),
    ("redstone", 4, 1),
    ("lapis_lazuli", 8, 2),
    ("quartz", 8, 2),
    ("emerald", 80, 20),
    ("diamond", 400, 100),
    // Wood.
    ("oak_log", 4, 1),
    ("spruce_log", 4, 1),
    ("birch_log", 4, 1),
    ("jungle_log", 4, 1),
    ("acacia_log", 4, 1),
    ("dark_oak_log", 4, 1),
    ("mangrove_log", 4, 1),
    ("cherry_log", 4, 1),
    // Mob drops.
    ("rotten_flesh", 0, 1),
    ("bone", 3, 1),
    ("string", 3, 1),
    ("gunpowder", 8, 2),
    ("spider_eye", 4, 1),
    ("slime_ball", 10, 2),
    ("blaze_rod", 30, 8),
    ("leather", 6, 2),
    ("feather", 2, 1),
    // Building.
    ("cobblestone", 1, 0),
    ("stone", 2, 0),
    ("dirt", 1, 0),
    ("sand", 2, 0),
    ("gravel", 2, 0),
    ("glass", 3, 0),
    ("bricks", 4, 0),
    ("stone_bricks", 3, 0),
    // Food.
    ("bread", 5, 0),
    ("cooked_beef", 6, 0),
    ("cooked_porkchop", 6, 0),
    ("cooked_chicken", 5, 0),
    ("baked_potato", 4, 0),
    ("apple", 4, 0),
    ("golden_carrot", 40, 0),
    // Tools.
    ("torch", 1, 0),
    ("ladder", 2, 0),
    ("crafting_table", 4, 0),
    ("chest", 6, 0),
    ("furnace", 6, 0),
];

impl Default for Config {
    /// `PRICES` (one price where the market trades both ways), without daily caps.
    fn default() -> Self {
        Self {
            enabled: true,
            items: PRICES
                .iter()
                .map(|(id, buy, sell)| {
                    let one = one_price(*buy, *sell);
                    (
                        format!("minecraft:{id}"),
                        Price {
                            buy: if *buy > 0 { one } else { 0 },
                            sell: if *sell > 0 { one } else { 0 },
                        },
                    )
                })
                .collect(),
            daily_sell_coins: 0,
            daily_sell_items: 0,
            dynamic: Dynamic::default(),
        }
    }
}

/// The default prices of SMP 0.1 to 0.11: a config that still holds
/// exactly these (never edited) gets today's defaults (`migrate`).
const OLD_PRICES: [(&str, i64, i64); 18] = [
    ("wheat", 0, 1),
    ("carrot", 0, 1),
    ("potato", 0, 1),
    ("sugar_cane", 0, 1),
    ("pumpkin", 0, 3),
    ("melon_slice", 0, 1),
    ("cactus", 0, 1),
    ("oak_log", 4, 1),
    ("coal", 8, 2),
    ("iron_ingot", 30, 8),
    ("gold_ingot", 40, 10),
    ("redstone", 4, 1),
    ("lapis_lazuli", 8, 2),
    ("emerald", 80, 20),
    ("diamond", 400, 100),
    ("bread", 5, 0),
    ("cooked_beef", 6, 0),
    ("torch", 1, 0),
];

/// A config whose prices are still the old defaults, untouched: today's
/// defaults instead. Whether it changed (the file is written again).
pub fn migrate(config: &mut Config) -> bool {
    let old: BTreeMap<String, Price> = OLD_PRICES
        .iter()
        .map(|(id, buy, sell)| {
            (
                format!("minecraft:{id}"),
                Price {
                    buy: *buy,
                    sell: *sell,
                },
            )
        })
        .collect();
    if config.items != old {
        return false;
    }
    config.items = Config::default().items;
    true
}

/// What one player sold today.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sold {
    /// The UTC day (days since 1970) the counts are for.
    pub day: i64,
    pub coins: i64,
    pub items: BTreeMap<String, u32>,
}

/// `market.json`: what each player sold today, by UUID.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ledger {
    pub sold: BTreeMap<String, Sold>,
}

/// A config's prices made safe: an amount out of range closes that side; an
/// item traded both ways gets one price; an item a chain of recipes makes
/// for less than it sells for is no longer bought. What changed, to log.
pub fn validate(config: &mut Config) -> Vec<String> {
    let mut closed = Vec::new();
    config.daily_sell_coins = config.daily_sell_coins.clamp(0, MAX_AMOUNT);
    config.dynamic.validate();
    for (id, p) in config.items.iter_mut() {
        if !valid_id(id) {
            closed.push(format!("{id}: not an item id; left out"));
            *p = Price::default();
            continue;
        }
        p.buy = p.buy.clamp(0, MAX_AMOUNT);
        p.sell = p.sell.clamp(0, MAX_AMOUNT);
    }
    // One price an item: where both sides are open they are the same
    // (before 0.29 the market bought for less than it sold).
    for (id, p) in config.items.iter_mut() {
        if p.buy > 0 && p.sell > 0 && p.buy != p.sell {
            let one = one_price(p.buy, p.sell);
            closed.push(format!(
                "{id}: one price now, {one} (it bought for {} and sold for {})",
                p.sell, p.buy
            ));
            p.buy = one;
            p.sell = one;
        }
    }
    // Chains of recipes at the base prices. Items made of one another share
    // one factor (`families`), so their prices keep these ratios as they
    // move: no chain can open later.
    let loops = super::smp_recipes::loops(&config.items, &super::smp_recipes::chains());
    for l in loops {
        closed.push(format!(
            "{}: crafted from the market's items for {:.2}, sells for {}; the market won't buy it",
            l.item, l.cost, l.sell
        ));
        if let Some(p) = config.items.get_mut(&l.item) {
            p.sell = 0;
        }
    }
    config.items.retain(|_, p| p.buy > 0 || p.sell > 0);
    closed
}

/// A price of one, for players: whole coins from 10 up, else to the
/// hundredth (`1.94 coins`).
pub fn each_money(economy: &Economy, each: f64) -> String {
    if each >= 10.0 || (each - each.round()).abs() < 0.005 {
        economy.money(each.round() as i64)
    } else {
        format!("{each:.2} {}", economy.config.currency)
    }
}

/// `minecraft:iron_ingot`: lowercase letters, digits, `_`, `.`, `-`, `/`
/// and one `:`, as a command line may carry it.
pub fn valid_id(id: &str) -> bool {
    let mut parts = id.split(':');
    let (Some(ns), Some(path), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let ok = |s: &str, extra: &str| {
        !s.is_empty()
            && s.chars().all(|c| {
                c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || "_.-".contains(c)
                    || extra.contains(c)
            })
    };
    ok(ns, "") && ok(path, "/")
}

/// The acting player's items, lent by the game for one command.
pub trait Bag {
    /// The main hand: item id, count, plain.
    fn held(&self) -> Option<(String, u32, bool)>;
    /// Every plain item carried, with how many.
    fn carried(&self) -> Vec<(String, u32)>;
    fn count(&mut self, item: &str) -> u32;
    /// Takes up to `count`; what it took.
    fn take(&mut self, item: &str, count: u32) -> u32;
    /// How many more fit.
    fn room(&mut self, item: &str) -> u32;
    /// Gives up to `count` where they fit; what it gave.
    fn give(&mut self, item: &str, count: u32) -> u32;
}

/// `diamond` or `minecraft:diamond` as `minecraft:diamond`.
pub fn item_id(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    if name.contains(':') {
        name
    } else {
        format!("minecraft:{name}")
    }
}

/// `minecraft:iron_ingot` as `iron ingot`.
pub fn item_name(id: &str) -> String {
    id.strip_prefix("minecraft:")
        .unwrap_or(id)
        .replace('_', " ")
}

pub struct Market {
    pub config: Config,
    ledger: Ledger,
    /// `market.json`; None in tests that keep nothing.
    path: Option<PathBuf>,
    dirty: bool,
    /// The moving prices and their history (`market_prices.json`, written
    /// by the glue off the tick thread: `book_to_write`).
    pub book: Book,
    book_written: i64,
    book_dirty: bool,
    /// For the server log (the glue takes them).
    pub warnings: Vec<String>,
    /// Each item made of or into others: those others (`families`).
    family: BTreeMap<String, Vec<String>>,
}

/// The market's items made of one another (by `smp_recipes::chains`), in
/// groups of two or more: they share one moving factor.
pub fn families(items: &BTreeMap<String, Price>) -> Vec<Vec<String>> {
    let mut group: BTreeMap<String, usize> = items
        .keys()
        .cloned()
        .enumerate()
        .map(|(i, k)| (k, i))
        .collect();
    let root = |g: &BTreeMap<String, usize>, id: &str| g.get(id).copied();
    for r in super::smp_recipes::chains() {
        for (choices, _) in &r.inputs {
            for c in choices {
                if let (Some(a), Some(b)) = (root(&group, &r.result), root(&group, c))
                    && a != b
                {
                    let (keep, gone) = (a.min(b), a.max(b));
                    for v in group.values_mut() {
                        if *v == gone {
                            *v = keep;
                        }
                    }
                }
            }
        }
    }
    let mut out: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (id, g) in group {
        out.entry(g).or_default().push(id);
    }
    out.into_values().filter(|f| f.len() > 1).collect()
}

/// How often the prices' file is written.
const BOOK_EVERY_MS: i64 = 5 * MINUTE_MS;

impl Market {
    /// The market of `dir` (`world/horizon-smp/`), its config made safe
    /// (`validate`), with what to log. A `market.json` that can't be read
    /// starts the day's counts from nothing (they only limit, never hold
    /// money).
    pub fn open(dir: &std::path::Path, mut config: Config) -> (Self, Vec<String>) {
        let mids = |c: &Config| -> BTreeMap<String, f64> {
            c.items
                .iter()
                .map(|(id, p)| (id.clone(), base_mid(p.buy, p.sell)))
                .collect()
        };
        let before = mids(&config);
        let mut problems: Vec<String> = validate(&mut config)
            .into_iter()
            .map(|l| format!("market: {l}"))
            .collect();
        let path = dir.join("market.json");
        let ledger = read_json(&path).unwrap_or_else(|e| {
            problems.push(format!("{e}: today's sale limits start again"));
            Ledger::default()
        });
        let mut book: Book = read_json(&dir.join("market_prices.json")).unwrap_or_else(|e| {
            problems.push(format!("{e}: prices start again from their base"));
            Book::default()
        });
        let rebased = book.settle(&before, &mids(&config));
        if !rebased.is_empty() {
            problems.push(format!(
                "market: the price history of {} items scaled to their one price",
                rebased.len()
            ));
        }
        let mut market = Self {
            config,
            ledger,
            path: Some(path),
            dirty: false,
            book,
            book_written: 0,
            book_dirty: !rebased.is_empty(),
            warnings: Vec::new(),
            family: BTreeMap::new(),
        };
        market.link();
        (market, problems)
    }

    /// A market that keeps nothing on disk, its config as given (tests).
    #[cfg(test)]
    pub fn new(config: Config) -> Self {
        let mut m = Self {
            config,
            ledger: Ledger::default(),
            path: None,
            dirty: false,
            book: Book::default(),
            book_written: 0,
            book_dirty: false,
            warnings: Vec::new(),
            family: BTreeMap::new(),
        };
        m.link();
        m
    }

    /// The families of the config's items, their factors made one.
    fn link(&mut self) {
        let groups = families(&self.config.items);
        self.book.link(&groups);
        self.family = groups
            .iter()
            .flat_map(|f| {
                f.iter()
                    .map(move |id| (id.clone(), f.iter().filter(|o| *o != id).cloned().collect()))
            })
            .collect();
    }

    /// How far one `id` moves the log of its factor: in a family, by value
    /// (a block of 9 moves it as 9 ingots do), so crafting can't beat it.
    fn unit_move(&self, id: &str) -> f64 {
        let d = &self.config.dynamic;
        let mid = |id: &str| {
            let b = self.base(id);
            base_mid(b.buy, b.sell)
        };
        match self
            .family
            .get(id)
            .and_then(|f| f.iter().chain([&id.to_owned()]).min().cloned())
        {
            Some(first) if mid(&first) > 0.0 => {
                Book::unit_move(d, &first, mid(&first)) / mid(&first) * mid(id)
            }
            _ => Book::unit_move(d, id, mid(id)),
        }
    }

    /// What `n` of `id` are worth now, bought (`buy`) or sold, in whole
    /// coins: along the price as the trade moves it (fixed prices: n times
    /// the price); all requested items are priced.
    pub fn value(&self, id: &str, n: u32, buy: bool) -> (i64, u32) {
        let b = self.base(id);
        let mid = base_mid(b.buy, b.sell);
        if !self.config.dynamic.enabled {
            return ((mid as i64).saturating_mul(i64::from(n)), n);
        }
        let (v, _, done) =
            self.book
                .fill(&self.config.dynamic, id, mid, self.unit_move(id), n, buy);
        (if buy { to_pay(v) } else { to_get(v) }, done)
    }

    /// The price of one `id` now (its mid), for display.
    pub fn each(&self, id: &str) -> f64 {
        self.spot(id).map_or(0.0, |q| q.mid)
    }

    pub fn save(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(path) = &self.path {
            write_json(path, &self.ledger)?;
        }
        self.dirty = false;
        Ok(())
    }

    /// Legacy query: sales and incoming payments are unlimited.
    pub fn coins_left(&self, _uuid: u128, _now: i64) -> Option<i64> {
        None
    }

    pub fn items_left(&self, _uuid: u128, _now: i64, _id: &str) -> Option<u32> {
        None
    }

    pub fn may_receive(&self, _uuid: u128, _amount: i64, _now: i64) -> Result<(), i64> {
        Ok(())
    }

    /// Records received coins in the legacy daily ledger.
    pub fn count_received(&mut self, uuid: u128, now: i64, coins: i64) {
        let day = now.div_euclid(DAY_MS);
        let s = self.ledger.sold.entry(uuid_string(uuid)).or_default();
        if s.day != day {
            *s = Sold {
                day,
                ..Sold::default()
            };
        }
        s.coins = s.coins.saturating_add(coins.max(0));
        self.ledger.sold.retain(|_, s| s.day == day);
        self.dirty = true;
    }

    /// Records a sale in the daily ledger.
    fn count_sale(&mut self, uuid: u128, now: i64, id: &str, n: u32, coins: i64) {
        let day = now.div_euclid(DAY_MS);
        let s = self.ledger.sold.entry(uuid_string(uuid)).or_default();
        if s.day != day {
            *s = Sold {
                day,
                ..Sold::default()
            };
        }
        s.coins = s.coins.saturating_add(coins);
        let c = s.items.entry(id.to_owned()).or_default();
        *c = c.saturating_add(n);
        // Yesterday's counts go.
        self.ledger.sold.retain(|_, s| s.day == day);
        self.dirty = true;
    }

    /// The price of `id` now (nothing on either side when it isn't
    /// traded): the base one moved by supply and demand.
    pub fn price(&self, id: &str) -> Price {
        let base = self.base(id);
        if !self.config.dynamic.enabled {
            return base;
        }
        let q = self.book.quote(
            &self.config.dynamic,
            id,
            base_mid(base.buy, base.sell),
            self.unit_move(id),
        );
        Price {
            buy: if base.buy > 0 { q.buy } else { 0 },
            sell: if base.sell > 0 { q.sell } else { 0 },
        }
    }

    /// The config's price of `id`, before supply and demand.
    pub fn base(&self, id: &str) -> Price {
        self.config.items.get(id).copied().unwrap_or_default()
    }

    /// What the market asks and pays for one `id` now, and the mid; None
    /// when it isn't traded. For the charts.
    pub fn spot(&self, id: &str) -> Option<Quote> {
        let b = self.config.items.get(id)?;
        Some(if self.config.dynamic.enabled {
            self.book.quote(
                &self.config.dynamic,
                id,
                base_mid(b.buy, b.sell),
                self.unit_move(id),
            )
        } else {
            Quote {
                mid: base_mid(b.buy, b.sell),
                buy: b.buy,
                sell: b.sell,
            }
        })
    }

    /// Items of `id` traded (both sides) in the last `hours`.
    pub fn volume(&self, id: &str, now: i64, hours: u32) -> u64 {
        self.book.volume(id, now, hours)
    }

    pub fn history(&self, id: &str, span: Span) -> &[Point] {
        self.book.history(id, span)
    }

    /// The change of `id`'s mid over the last `minutes`, in percent.
    pub fn change(&self, id: &str, now: i64, minutes: u32) -> Option<f64> {
        let mid = self.spot(id)?.mid;
        self.book.change(id, now, minutes, mid)
    }

    /// The minute timer, called once a second: whether prices moved.
    pub fn tick(&mut self, now: i64) -> bool {
        if !self.config.enabled || !self.config.dynamic.enabled {
            return false;
        }
        let mids: BTreeMap<String, f64> = self
            .config
            .items
            .iter()
            .map(|(id, p)| (id.clone(), base_mid(p.buy, p.sell)))
            .collect();
        let moved = self.book.tick(&self.config.dynamic, &mids, now);
        self.book_dirty |= moved;
        moved
    }

    /// The prices' history to write, every 5 minutes when it changed (and
    /// always with `force`): its path and a copy, for a thread to write.
    pub fn book_to_write(&mut self, now: i64, force: bool) -> Option<(PathBuf, Book)> {
        if !self.book_dirty || (!force && now - self.book_written < BOOK_EVERY_MS) {
            return None;
        }
        let path = self.path.as_ref()?.with_file_name("market_prices.json");
        self.book_dirty = false;
        self.book_written = now;
        Some((path, self.book.clone()))
    }

    /// The day's movers and news, once per UTC day (at its first minute):
    /// None until then.
    pub fn summary_due(&mut self, now: i64) -> Option<Vec<String>> {
        let day = now.div_euclid(DAY_MS);
        if !self.config.dynamic.enabled || self.book.summary_day == day {
            return None;
        }
        let first = self.book.summary_day == 0;
        self.book.summary_day = day;
        self.book_dirty = true;
        if first {
            return None;
        }
        let items: Vec<(String, String, f64)> = self
            .config
            .items
            .keys()
            .filter_map(|id| Some((id.clone(), title_name(id), self.spot(id)?.mid)))
            .collect();
        let lines = self.book.summary(&self.config.dynamic, &items, now);
        self.book.last_summary.clone_from(&lines);
        Some(lines)
    }

    /// One line on an item's move: `▲ 2.4% 1h, ▼ 0.3% 24h`.
    pub fn changes_line(&self, id: &str, now: i64) -> String {
        let c = |m| {
            self.change(id, now, m)
                .map_or_else(|| "–".to_owned(), arrow)
        };
        format!("{} 1h, {} 24h", c(60), c(24 * 60))
    }

    /// A copy of what `web/market.json` is made of, for a thread to build
    /// and write it (the tick only copies).
    pub fn web_view(&self) -> WebView {
        WebView {
            config: self.config.clone(),
            book: self.book.clone(),
            deltas: self
                .config
                .items
                .keys()
                .map(|id| (id.clone(), self.unit_move(id)))
                .collect(),
        }
    }

    /// Kept for callers of the old limits display.
    pub fn limits_line(&self, _economy: &Economy, _uuid: u128) -> Option<String> {
        None
    }

    /// Runs `/worth`, `/sell` or `/buy`; None for another command. `trades`
    /// is false for a player in creative or spectator mode.
    pub fn command(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        trades: bool,
        bag: &mut dyn Bag,
        line: &str,
    ) -> Option<Vec<Out>> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let (&name, args) = words.split_first()?;
        if !matches!(name, "worth" | "sell" | "buy") {
            return None;
        }
        if !self.config.enabled {
            return Some(vec![Out::Error("The market is closed.".into())]);
        }
        if name == "worth" {
            return Some(self.worth(economy, me, bag, args));
        }
        if !trades {
            return Some(vec![Out::Error(
                "The market trades only with players in survival or adventure mode.".into(),
            )]);
        }
        Some(match name {
            "sell" => self.sell(economy, me, bag, args),
            _ => self.buy(economy, me, bag, args),
        })
    }

    fn worth(&self, economy: &Economy, me: &Player, bag: &mut dyn Bag, args: &[&str]) -> Vec<Out> {
        let id = match args {
            [] => match bag.held() {
                Some((id, ..)) => id,
                None => {
                    return vec![Out::Error(
                        "Hold an item, or name one: /worth <item>".into(),
                    )];
                }
            },
            [item] => item_id(item),
            _ => return vec![Out::Error("Usage: /worth [item]".into())],
        };
        let p = self.price(&id);
        let name = item_name(&id);
        let ways = match (p.buy > 0, p.sell > 0) {
            (true, true) => "to buy or sell",
            (true, false) => "to buy (the market doesn't buy it)",
            (false, true) => "to sell (the market doesn't sell it)",
            (false, false) => return vec![Out::Reply(format!("The market doesn't trade {name}."))],
        };
        let mut out = vec![Out::Reply(format!(
            "{name}: {} each, {ways}.",
            each_money(economy, self.each(&id))
        ))];
        if self.config.dynamic.enabled {
            out.push(Out::Reply(format!(
                "Moves: {}; traded in 24 h: {}. Chart: /market {}",
                self.changes_line(&id, economy.now),
                self.volume(&id, economy.now, 24),
                id.trim_start_matches("minecraft:")
            )));
        }
        if p.sell > 0 {
            if let Some(left) = self.items_left(me.uuid, economy.now, &id) {
                out.push(Out::Reply(format!(
                    "You may sell {left} more {name} today."
                )));
            }
            out.extend(self.limits_line(economy, me.uuid).map(Out::Reply));
        }
        out
    }

    /// Sells what `args` names (`hand [n]`, `all`, `<item> [n]`).
    pub fn sell(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        bag: &mut dyn Bag,
        args: &[&str],
    ) -> Vec<Out> {
        let usage = || vec![Out::Error("Usage: /sell hand|all|<item> [count]".into())];
        // What to sell: (item, at most how many).
        let wanted: Vec<(String, u32)> = match args {
            ["all"] => bag
                .carried()
                .into_iter()
                .filter(|(id, _)| self.price(id).sell > 0)
                .collect(),
            ["hand", rest @ ..] => {
                let Some((id, count, plain)) = bag.held() else {
                    return vec![Out::Error("Your hand is empty.".into())];
                };
                if !plain {
                    return vec![Out::Error(
                        "The market only takes plain items (not worn, named or enchanted).".into(),
                    )];
                }
                match rest {
                    [] => vec![(id, count)],
                    [n] => match parse_count(n) {
                        Some(n) => vec![(id, n.min(count))],
                        None => return usage(),
                    },
                    _ => return usage(),
                }
            }
            [item] => vec![(item_id(item), u32::MAX)],
            [item, n] => match parse_count(n) {
                Some(n) => vec![(item_id(item), n)],
                None => return usage(),
            },
            _ => return usage(),
        };
        if wanted.is_empty() {
            return vec![Out::Error("You carry nothing the market buys.".into())];
        }
        let mut out = Vec::new();
        let mut total = 0i64;
        for (id, at_most) in wanted {
            let name = item_name(&id);
            if self.base(&id).sell <= 0 {
                out.push(Out::Error(format!("The market doesn't buy {name}.")));
                continue;
            }
            let price = self.each(&id).max(0.01);
            // Keep the payment within the economy's numeric range.
            let now = economy.now;
            let cap = (MAX_AMOUNT as f64 / price).min(f64::from(u32::MAX)) as u32;
            let carried = bag.count(&id).min(at_most);
            if carried == 0 {
                out.push(Out::Error(format!("You have no plain {name}.")));
                continue;
            }
            let limited = carried.min(cap);
            if limited == 0 {
                out.push(Out::Error(format!(
                    "The value of {name} exceeds the supported payment range."
                )));
                continue;
            }
            let have = limited;
            let taken = bag.take(&id, have);
            if taken == 0 {
                continue;
            }
            let (pay, done) = self.value(&id, taken, false);
            let taken = done;
            if taken == 0 {
                continue;
            }
            if pay <= 0 {
                bag.give(&id, taken);
                out.push(Out::Error(format!(
                    "{taken} {name} are worth less than a coin now; kept."
                )));
                continue;
            }
            let kind = format!("sell {taken} {id}");
            match economy.adjust(&kind, me.uuid, pay, "") {
                Ok(_) => {
                    self.count_sale(me.uuid, now, &id, taken, pay);
                    self.moved(&id, taken, false);
                    total = total.saturating_add(pay);
                    let each = each_money(economy, pay as f64 / f64::from(taken));
                    out.push(Out::Reply(format!(
                        "Sold {taken} {name} at {each} each: {}.",
                        economy.money(pay)
                    )));
                    if limited < carried {
                        out.push(Out::Reply(format!(
                            "{} {name} kept: the payment exceeds the supported numeric range.",
                            carried - limited
                        )));
                    }
                }
                Err(e) => {
                    let back = bag.give(&id, taken);
                    out.push(Out::Error(format!(
                        "The sale failed ({e}); {back} {name} given back."
                    )));
                }
            }
        }
        if total > 0 {
            out.push(Out::Reply(format!(
                "Balance: {}",
                economy.money(economy.balance(me.uuid))
            )));
        }
        out
    }

    /// Buys `args` (`<item> [count]`).
    pub fn buy(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        bag: &mut dyn Bag,
        args: &[&str],
    ) -> Vec<Out> {
        let (item, count) = match args {
            [item] => (*item, 1),
            [item, n] => match parse_count(n) {
                Some(n) => (*item, n),
                None => return vec![Out::Error("Usage: /buy <item> [count]".into())],
            },
            _ => return vec![Out::Error("Usage: /buy <item> [count]".into())],
        };
        let id = item_id(item);
        let name = item_name(&id);
        if self.base(&id).buy <= 0 {
            return vec![Out::Error(format!("The market doesn't sell {name}."))];
        }
        if count == 0 {
            return vec![Out::Error("Usage: /buy <item> [count]".into())];
        }
        let (cost, done) = self.value(&id, count, true);
        let count = done;
        if cost > MAX_AMOUNT {
            return vec![Out::Error("That costs too much at once.".into())];
        }
        if economy.balance(me.uuid) < cost {
            return vec![Out::Error(format!(
                "{count} {name} cost {}; you have {}.",
                economy.money(cost),
                economy.money(economy.balance(me.uuid))
            ))];
        }
        let room = bag.room(&id);
        if room < count {
            return vec![Out::Error(format!(
                "Only {room} {name} fit in your inventory."
            ))];
        }
        if let Err(e) = economy.adjust(&format!("buy {count} {id}"), me.uuid, -cost, "") {
            return vec![Out::Error(e)];
        }
        let given = bag.give(&id, count);
        // What was given, valued before the price moves.
        let paid = if given == count {
            cost
        } else {
            self.value(&id, given, true).0
        };
        if given > 0 {
            self.moved(&id, given, true);
            let now = economy.now;
            self.book.bought(&uuid_string(me.uuid), &id, given, now);
            self.book_dirty = true;
        }
        let each = each_money(economy, paid as f64 / f64::from(given.max(1)));
        let mut out = vec![Out::Reply(format!(
            "Bought {given} {name} at {each} each: {}.",
            economy.money(paid)
        ))];
        if given < count {
            // Checked above; kept so money never goes for nothing.
            let refund = cost - paid;
            if economy
                .adjust(
                    &format!("refund {} {id}", count - given),
                    me.uuid,
                    refund,
                    "",
                )
                .is_ok()
            {
                out.push(Out::Reply(format!(
                    "{} refunded for what didn't fit.",
                    economy.money(refund)
                )));
            }
        }
        out.push(Out::Reply(format!(
            "Balance: {}",
            economy.money(economy.balance(me.uuid))
        )));
        out
    }
}

impl Market {
    /// A trade's move of the price (at once, with its family's).
    fn moved(&mut self, id: &str, n: u32, buy: bool) {
        if !self.config.dynamic.enabled {
            return;
        }
        let b = self.base(id);
        let delta = self.unit_move(id);
        let family = self.family.get(id).cloned().unwrap_or_default();
        self.book.trade(
            &self.config.dynamic,
            id,
            &family,
            base_mid(b.buy, b.sell),
            delta,
            n,
            buy,
        );
        self.book_dirty = true;
    }

    /// Purchases may be resold immediately, including with legacy configs.
    pub fn recently_bought(&self, _uuid: u128, _id: &str, _now: i64) -> u32 {
        0
    }
}

/// `minecraft:iron_ingot` as `Iron Ingot`.
pub fn title_name(id: &str) -> String {
    item_name(id)
        .split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn parse_count(s: &str) -> Option<u32> {
    parse_amount(s).and_then(|n| u32::try_from(n).ok())
}

/// The market as `web/market.json` shows it (built off the tick thread).
pub struct WebView {
    config: Config,
    book: Book,
    /// Each item's move a unit (families by value).
    deltas: BTreeMap<String, f64>,
}

/// A price rounded to the cent, for the web.
fn cents(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Points grouped by `step` (ms): each group's first time, last price and
/// summed volume.
fn series(points: &[Point], step: i64) -> serde_json::Value {
    let mut out: Vec<(i64, f64, u64)> = Vec::new();
    for p in points {
        let t = p.t.div_euclid(step) * step;
        match out.last_mut() {
            Some(last) if last.0 == t => {
                last.1 = p.price;
                last.2 += u64::from(p.volume);
            }
            _ => out.push((t, p.price, u64::from(p.volume))),
        }
    }
    serde_json::json!({
        "step": step,
        "t": out.iter().map(|p| p.0).collect::<Vec<_>>(),
        "price": out.iter().map(|p| cents(p.1)).collect::<Vec<_>>(),
        "volume": out.iter().map(|p| p.2).collect::<Vec<_>>(),
    })
}

impl WebView {
    /// `{"schema":1,"generated",...}`: each item's prices now, its moves,
    /// its 24 h volume, its last day in 5-minute points and its last 30
    /// days in hours. `categories`: (name, items) in menu order.
    pub fn doc(
        &self,
        categories: &[(String, Vec<String>)],
        currency: &str,
        now: i64,
    ) -> serde_json::Value {
        let d = &self.config.dynamic;
        let items: Vec<serde_json::Value> = self
            .config
            .items
            .iter()
            .map(|(id, base)| {
                let q = if d.enabled {
                    let mid = base_mid(base.buy, base.sell);
                    let delta = self
                        .deltas
                        .get(id)
                        .copied()
                        .unwrap_or_else(|| Book::unit_move(d, id, mid));
                    self.book.quote(d, id, mid, delta)
                } else {
                    Quote {
                        mid: base_mid(base.buy, base.sell),
                        buy: base.buy,
                        sell: base.sell,
                    }
                };
                let change = |m| self.book.change(id, now, m, q.mid).map(cents);
                let category = categories
                    .iter()
                    .find(|c| c.1.contains(id))
                    .map(|c| c.0.as_str());
                serde_json::json!({
                    "id": id,
                    "name": title_name(id),
                    "category": category,
                    "price": cents(q.mid),
                    // The same one price, on the sides the market trades
                    // (for readers of the older buy/sell pair).
                    "buy": (base.buy > 0).then_some(cents(q.mid)),
                    "sell": (base.sell > 0).then_some(cents(q.mid)),
                    "mid": cents(q.mid),
                    "base": cents(base_mid(base.buy, base.sell)),
                    "change_1h": change(60),
                    "change_24h": change(24 * 60),
                    "volume_24h": self.book.volume(id, now, 24),
                    "day": series(self.book.history(id, Span::Minute), 5 * MINUTE_MS),
                    "month": series(self.book.history(id, Span::Hour), 60 * MINUTE_MS),
                })
            })
            .collect();
        serde_json::json!({
            "schema": 1,
            "generated": now,
            "currency": currency,
            "moving": self.config.enabled && d.enabled,
            // No tax since 0.30: kept at 0 for older readers.
            "tax_percent": 0,
            "daily_summary": {
                "day": self.book.summary_day * DAY_MS,
                "lines": self.book.last_summary,
            },
            "items": items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::smp_economy::Config as EconomyConfig;
    use super::*;
    use std::fs;

    /// A pretend inventory: plain stacks by item, one held item, a room cap.
    struct TestBag {
        items: BTreeMap<String, u32>,
        held: Option<(String, u32, bool)>,
        room: u32,
    }

    impl Bag for TestBag {
        fn held(&self) -> Option<(String, u32, bool)> {
            self.held.clone()
        }
        fn carried(&self) -> Vec<(String, u32)> {
            self.items.iter().map(|(k, v)| (k.clone(), *v)).collect()
        }
        fn count(&mut self, item: &str) -> u32 {
            self.items.get(item).copied().unwrap_or(0)
        }
        fn take(&mut self, item: &str, count: u32) -> u32 {
            let have = self.items.entry(item.into()).or_default();
            let n = (*have).min(count);
            *have -= n;
            n
        }
        fn room(&mut self, _item: &str) -> u32 {
            self.room
        }
        fn give(&mut self, item: &str, count: u32) -> u32 {
            let n = count.min(self.room);
            self.room -= n;
            *self.items.entry(item.into()).or_default() += n;
            n
        }
    }

    const ALEX: u128 = 1;

    /// The fixed prices of SMP 0.15 (no supply and demand, no tax).
    fn fixed() -> Config {
        let mut c = Config::default();
        c.dynamic.enabled = false;
        c
    }

    fn setup(name: &str) -> (Economy, Market, Player, std::path::PathBuf) {
        setup_with(name, fixed())
    }

    fn setup_with(name: &str, config: Config) -> (Economy, Market, Player, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("smp-market-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut e = Economy::open(&dir, EconomyConfig::default()).unwrap();
        e.seen(ALEX, "Alex");
        let me = Player {
            uuid: ALEX,
            name: "Alex".into(),
            level: 0,
        };
        (e, Market::new(config), me, dir)
    }

    fn bag(items: &[(&str, u32)], room: u32) -> TestBag {
        TestBag {
            items: items.iter().map(|(k, v)| ((*k).into(), *v)).collect(),
            held: None,
            room,
        }
    }

    #[test]
    fn payments_ignore_legacy_daily_limits() {
        let config = Config {
            daily_sell_coins: 250,
            ..Config::default()
        };
        let (_e, mut m, _me, dir) = setup_with("received", config);
        m.count_received(ALEX, 0, 1_000_000);
        assert_eq!(m.coins_left(ALEX, 0), None);
        assert_eq!(m.may_receive(ALEX, 1_000_000, 0), Ok(()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn selling_takes_the_items_then_pays_and_logs_both() {
        let (mut e, mut m, me, dir) = setup("sell");
        let mut b = bag(
            &[
                ("minecraft:diamond", 3),
                ("minecraft:wheat", 20),
                ("minecraft:dirt", 64),
            ],
            0,
        );
        let out = m
            .command(&mut e, &me, true, &mut b, "sell diamond 2")
            .unwrap();
        assert_eq!(
            out[0],
            Out::Reply("Sold 2 diamond at 200 coins each: 400 coins.".into())
        );
        assert_eq!(b.items["minecraft:diamond"], 1);
        let out = m.command(&mut e, &me, true, &mut b, "sell all").unwrap();
        assert!(
            out.contains(&Out::Reply(
                "Sold 1 diamond at 200 coins each: 200 coins.".into()
            )),
            "{out:?}"
        );
        assert!(
            out.contains(&Out::Reply(
                "Sold 20 wheat at 2 coins each: 40 coins.".into()
            )),
            "{out:?}"
        );
        assert_eq!(b.items["minecraft:dirt"], 64, "dirt has no price");
        assert_eq!(e.balance(ALEX), 640);
        let log = fs::read_to_string(dir.join("audit.log")).unwrap();
        assert_eq!(log.matches("\"sell ").count(), 3, "{log}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn selling_the_hand_takes_only_plain_items() {
        let (mut e, mut m, me, dir) = setup("hand");
        let mut b = bag(&[("minecraft:iron_ingot", 10)], 0);
        b.held = Some(("minecraft:iron_ingot".into(), 10, true));
        let out = m.command(&mut e, &me, true, &mut b, "sell hand 4").unwrap();
        assert_eq!(
            out[0],
            Out::Reply("Sold 4 iron ingot at 15 coins each: 60 coins.".into())
        );
        b.held = Some(("minecraft:diamond_sword".into(), 1, false));
        let out = m.command(&mut e, &me, true, &mut b, "sell hand").unwrap();
        assert_eq!(
            out,
            [Out::Error(
                "The market only takes plain items (not worn, named or enchanted).".into()
            )]
        );
        b.held = None;
        let out = m.command(&mut e, &me, true, &mut b, "sell hand").unwrap();
        assert_eq!(out, [Out::Error("Your hand is empty.".into())]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn buying_checks_money_and_room_before_anything_moves() {
        let (mut e, mut m, me, dir) = setup("buy");
        e.adjust("test", ALEX, 1000, "").unwrap();
        let mut b = bag(&[], 3);
        let out = m
            .command(&mut e, &me, true, &mut b, "buy diamond 6")
            .unwrap();
        assert_eq!(
            out,
            [Out::Error(
                "6 diamond cost 1,200 coins; you have 1,000 coins.".into()
            )]
        );
        let out = m.command(&mut e, &me, true, &mut b, "buy torch 4").unwrap();
        assert_eq!(
            out,
            [Out::Error("Only 3 torch fit in your inventory.".into())]
        );
        assert_eq!(e.balance(ALEX), 1000);
        let out = m
            .command(&mut e, &me, true, &mut b, "buy minecraft:diamond 2")
            .unwrap();
        assert_eq!(
            out[0],
            Out::Reply("Bought 2 diamond at 200 coins each: 400 coins.".into())
        );
        assert_eq!((e.balance(ALEX), b.items["minecraft:diamond"]), (600, 2));
        let out = m
            .command(&mut e, &me, true, &mut b, "buy rotten_flesh")
            .unwrap();
        assert_eq!(
            out,
            [Out::Error("The market doesn't sell rotten flesh.".into())]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn creative_players_and_a_closed_market_trade_nothing() {
        let (mut e, mut m, me, dir) = setup("closed");
        let mut b = bag(&[("minecraft:diamond", 64)], 64);
        let out = m.command(&mut e, &me, false, &mut b, "sell all").unwrap();
        assert_eq!(
            out,
            [Out::Error(
                "The market trades only with players in survival or adventure mode.".into()
            )]
        );
        let worth = m
            .command(&mut e, &me, false, &mut b, "worth diamond")
            .unwrap();
        assert_eq!(
            worth[0],
            Out::Reply("diamond: 200 coins each, to buy or sell.".into())
        );
        m.config.enabled = false;
        let out = m.command(&mut e, &me, true, &mut b, "sell all").unwrap();
        assert_eq!(out, [Out::Error("The market is closed.".into())]);
        assert_eq!((e.balance(ALEX), b.items["minecraft:diamond"]), (0, 64));
        assert!(m.command(&mut e, &me, true, &mut b, "balance").is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sales_ignore_legacy_daily_limits_even_after_restart() {
        let config = Config {
            daily_sell_coins: 250,
            daily_sell_items: 30,
            ..fixed()
        };
        let (mut e, mut m, me, dir) = setup_with("limits", config);
        let mut b = bag(&[("minecraft:diamond", 64), ("minecraft:wheat", 640)], 0);
        m.command(&mut e, &me, true, &mut b, "sell all").unwrap();
        assert_eq!(e.balance(me.uuid), 64 * 200 + 640 * 2);
        assert_eq!(b.count("minecraft:diamond"), 0);
        assert_eq!(b.count("minecraft:wheat"), 0);
        assert_eq!(m.items_left(me.uuid, 0, "minecraft:diamond"), None);
        assert_eq!(m.limits_line(&e, me.uuid), None);
        m.path = Some(dir.join("market.json"));
        m.save().unwrap();
        let (mut m, _) = Market::open(&dir, m.config.clone());
        b.items.insert("minecraft:diamond".into(), 64);
        m.command(&mut e, &me, true, &mut b, "sell all").unwrap();
        assert_eq!(e.balance(me.uuid), 128 * 200 + 640 * 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prices_that_would_make_money_are_closed() {
        let mut config = fixed();
        let set = |c: &mut Config, id: &str, buy, sell| {
            c.items
                .insert(format!("minecraft:{id}"), Price { buy, sell });
        };
        set(&mut config, "emerald", 20, 20);
        set(&mut config, "iron_block", 0, 300);
        set(&mut config, "gold_block", 0, 180);
        set(&mut config, "rotten_flesh", -5, i64::MAX);
        config.items.insert(
            "minecraft:diamond 1\nop me".into(),
            Price { buy: 1, sell: 0 },
        );
        let closed = validate(&mut config);
        assert_eq!(closed.len(), 2, "{closed:?}");
        assert_eq!(
            config.items["minecraft:emerald"],
            Price { buy: 20, sell: 20 }
        );
        // 9 ingots at 15 make a block: sold for 300, a loop; gold's 9 at 20
        // are 180, sold for 180: no gain.
        assert!(!config.items.contains_key("minecraft:iron_block"));
        assert_eq!(config.items["minecraft:gold_block"].sell, 180);
        assert_eq!(
            config.items["minecraft:rotten_flesh"],
            Price {
                buy: 0,
                sell: MAX_AMOUNT
            }
        );
        assert!(config.items.keys().all(|k| valid_id(k)));
        // At the largest price: one at a time, without overflow.
        let (mut e, mut m, me, dir) = setup_with(
            "validate",
            Config {
                daily_sell_coins: 0,
                ..config
            },
        );
        let mut b = bag(&[("minecraft:rotten_flesh", 64)], 0);
        let out = m
            .command(&mut e, &me, true, &mut b, "sell rotten_flesh")
            .unwrap();
        assert_eq!(
            out[0],
            Out::Reply(format!(
                "Sold 1 rotten flesh at {} each: {}.",
                e.money(MAX_AMOUNT),
                e.money(MAX_AMOUNT)
            )),
            "{out:?}"
        );
        assert_eq!(b.items["minecraft:rotten_flesh"], 63);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_purchase_too_dear_to_count_is_refused() {
        let mut config = fixed();
        config.items.insert(
            "minecraft:diamond".into(),
            Price {
                buy: MAX_AMOUNT,
                sell: 0,
            },
        );
        let (mut e, mut m, me, dir) = setup_with("dear", config);
        let mut b = bag(&[], 64 * 36);
        let out = m
            .command(&mut e, &me, true, &mut b, "buy diamond 2")
            .unwrap();
        assert_eq!(out, [Out::Error("That costs too much at once.".into())]);
        let out = m
            .command(&mut e, &me, true, &mut b, "buy diamond 0")
            .unwrap();
        assert!(matches!(&out[0], Out::Error(_)), "{out:?}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_old_default_prices_become_the_new_ones_and_edited_ones_stay() {
        let mut config = Config {
            items: OLD_PRICES
                .iter()
                .map(|(id, buy, sell)| {
                    (
                        format!("minecraft:{id}"),
                        Price {
                            buy: *buy,
                            sell: *sell,
                        },
                    )
                })
                .collect(),
            ..fixed()
        };
        let mut edited = config.clone();
        assert!(migrate(&mut config));
        assert_eq!(config.items, Config::default().items);
        assert!(!migrate(&mut config), "once");
        edited.items.get_mut("minecraft:diamond").unwrap().sell = 90;
        let before = edited.clone();
        assert!(!migrate(&mut edited));
        assert_eq!(edited, before);
    }

    /// Moving prices, validated, with no daily caps and no round-trip
    /// window: only the prices themselves stand between a trader and money.
    fn moving() -> Config {
        let mut c = Config {
            daily_sell_coins: 0,
            daily_sell_items: 0,
            ..Config::default()
        };
        c.dynamic.round_trip_hours = 0;
        let _ = validate(&mut c);
        c
    }

    #[test]
    fn no_trades_on_moving_prices_ever_earn_once_the_items_are_back() {
        // Five players pump and dump every item for 2,000 minutes, with the
        // drift between; then every position is closed. On every item the
        // market ends up with the money.
        let c = moving();
        let mut m = Market::new(c.clone());
        let t0 = 1_800_000_000_000i64;
        let mut seed = 12345u64;
        let mut rand = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let ids: Vec<String> = c.items.keys().cloned().collect();
        // Per item: coins the players got (negative: paid), items they hold.
        let mut book: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        m.tick(t0);
        for minute in 1..2000i64 {
            let pump = (minute / 200) % 2 == 0;
            for _ in 0..5 {
                let id = &ids[rand(ids.len() as u64) as usize];
                let p = m.base(id);
                let n = 1 + rand(640) as u32;
                let e = book.entry(id.clone()).or_default();
                let buy = p.buy > 0 && (p.sell == 0 || pump || rand(4) == 0 || e.1 < i64::from(n));
                if !buy && (p.sell == 0 || e.1 < i64::from(n)) {
                    continue;
                }
                let (v, n) = m.value(id, n, buy);
                e.0 += if buy { -v } else { v };
                e.1 += if buy { i64::from(n) } else { -i64::from(n) };
                m.moved(id, n, buy);
            }
            m.tick(t0 + minute * MINUTE_MS);
        }
        for (id, (mut coins, held)) in book {
            let f = m.book.factor(&id);
            assert!(
                f >= c.dynamic.min_factor - 1e-9 && f <= c.dynamic.max_factor + 1e-9,
                "{id} {f}"
            );
            // Sold back a minute at a time, as the band allows.
            let (mut held, mut now) = (held, t0 + 2000 * MINUTE_MS);
            while held > 0 && m.base(&id).sell > 0 {
                let (v, n) = m.value(&id, held as u32, false);
                coins += v;
                held -= i64::from(n);
                m.moved(&id, n, false);
                now += MINUTE_MS;
                m.tick(now);
            }
            assert!(coins <= 0, "{id}: the players made {coins}");
        }
    }

    #[test]
    fn a_block_of_nine_is_nine_ingots_on_the_curve_and_crafting_never_pays() {
        // An op adds iron blocks at nine ingots' price: one family.
        let mut c = moving();
        c.items.insert(
            "minecraft:iron_block".into(),
            Price {
                buy: 135,
                sell: 135,
            },
        );
        assert!(validate(&mut c).is_empty() || c.items.contains_key("minecraft:iron_block"));
        let mut m = Market::new(c);
        assert!(m.family["minecraft:iron_ingot"].contains(&"minecraft:iron_block".to_string()));
        m.tick(1_800_000_000_000);
        for n in [1u32, 7, 14] {
            // Buy 9n ingots, craft n blocks, sell them; and the other way.
            let (paid, done) = m.value("minecraft:iron_ingot", 9 * n, true);
            assert_eq!(done, 9 * n);
            m.moved("minecraft:iron_ingot", 9 * n, true);
            let (got, _) = m.value("minecraft:iron_block", n, false);
            m.moved("minecraft:iron_block", n, false);
            assert!(got <= paid && paid - got <= 2, "{n}: {paid} {got}");
            let (paid, _) = m.value("minecraft:iron_block", n, true);
            m.moved("minecraft:iron_block", n, true);
            let (got, _) = m.value("minecraft:iron_ingot", 9 * n, false);
            m.moved("minecraft:iron_ingot", 9 * n, false);
            assert!(got <= paid && paid - got <= 2, "{n}: {paid} {got}");
            assert_eq!(
                m.book.factor("minecraft:iron_block"),
                m.book.factor("minecraft:iron_ingot")
            );
        }
        // A block priced above nine ingots is a loop: no longer bought.
        let mut c = moving();
        c.items
            .insert("minecraft:iron_block".into(), Price { buy: 0, sell: 200 });
        let log = validate(&mut c);
        assert!(
            log.iter()
                .any(|l| l.starts_with("minecraft:iron_block: crafted")),
            "{log:?}"
        );
        assert!(!c.items.contains_key("minecraft:iron_block"));
    }

    #[test]
    fn a_whale_who_pumps_a_price_and_sells_into_it_loses_money() {
        let c = moving();
        let (mut e, _, me, dir) = setup("whale");
        let mut m = Market::new(c);
        let t0 = 1_800_000_000_000i64;
        e.now = t0;
        e.adjust("test_funds", me.uuid, 50_000_000, "").unwrap();
        let start = e.balance(me.uuid);
        let mut b = bag(&[], 64 * 36);
        m.tick(t0);
        // Two hours of buying diamonds every minute: the price climbs (5%
        // a minute at most: the rest of the 64 waits).
        for minute in 1..=120 {
            let out = m.buy(&mut e, &me, &mut b, &["diamond", "64"]);
            assert!(
                matches!(&out[0], Out::Reply(r) if r.starts_with("Bought "))
                    || matches!(&out[0], Out::Error(r) if r.contains("as far as it may")),
                "{out:?}"
            );
            let now = t0 + minute * MINUTE_MS;
            e.now = now;
            m.tick(now);
            b.room = 64 * 36;
        }
        assert!(
            m.book.factor("minecraft:diamond") > 1.5,
            "the price was pushed up"
        );
        // Then sell it all into the price they pushed, a minute at a time.
        let mut now = t0 + 121 * MINUTE_MS;
        while b.items["minecraft:diamond"] > 0 {
            let out = m.sell(&mut e, &me, &mut b, &["diamond"]);
            assert!(
                out.iter()
                    .any(|o| matches!(o, Out::Reply(r) if r.starts_with("Sold "))),
                "{out:?}"
            );
            now += MINUTE_MS;
            e.now = now;
            m.tick(now);
        }
        assert!(
            e.balance(me.uuid) < start,
            "{} >= {start}",
            e.balance(me.uuid)
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn moving_prices_pay_the_whole_receipt_and_allow_immediate_resale() {
        let mut c = Config::default();
        let _ = validate(&mut c);
        let (mut e, _, me, dir) = setup("dyn");
        let mut m = Market::new(c);
        let t0 = 1_800_000_000_000i64;
        e.now = t0;
        e.adjust("test_funds", me.uuid, 10_000, "").unwrap();
        let mut b = bag(&[("minecraft:diamond", 3)], 64 * 36);
        // One price, 200; three diamonds move it 1.5% as they go.
        let out = m.sell(&mut e, &me, &mut b, &["diamond"]);
        assert_eq!(
            out[0],
            Out::Reply("Sold 3 diamond at 198 coins each: 595 coins.".into())
        );
        assert_eq!(e.balance(me.uuid), 10_000 + 595, "no tax taken");
        let out = m.buy(&mut e, &me, &mut b, &["iron_ingot", "10"]);
        assert_eq!(
            out[0],
            Out::Reply("Bought 10 iron ingot at 15 coins each: 151 coins.".into())
        );
        assert_eq!(e.balance(me.uuid), 10_000 + 595 - 151);
        let out = m.sell(&mut e, &me, &mut b, &["iron_ingot"]);
        assert!(
            matches!(&out[0], Out::Reply(r) if r.starts_with("Sold 10 iron ingot")),
            "{out:?}"
        );
        assert_eq!(b.count("minecraft:iron_ingot"), 0);
        assert_eq!(m.recently_bought(me.uuid, "minecraft:iron_ingot", t0), 0);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_history_from_before_one_price_is_scaled_to_it_once() {
        // 0.27's book: diamond at 1.1 times its old mid of 250 (buy 400,
        // sell 100), the minute band never set.
        let dir = std::env::temp_dir().join(format!("smp-one-price-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = r#"{"items":{"minecraft:diamond":{"factor":1.1,"minutes":[[0,275.0,3,1]],"hours":[[0,250.0,9,4]]}}}"#;
        std::fs::write(dir.join("market_prices.json"), old).unwrap();
        let mut c = Config::default();
        c.items.insert(
            "minecraft:diamond".into(),
            Price {
                buy: 400,
                sell: 100,
            },
        );
        let (m, log) = Market::open(&dir, c.clone());
        let t = &m.book.items["minecraft:diamond"];
        // Now quoted in the one price, 200: 1.1 times it, no cliff.
        assert!((t.minutes[0].price - 220.0).abs() < 1e-9, "{:?}", t.minutes);
        assert!((t.hours[0].price - 200.0).abs() < 1e-9, "{:?}", t.hours);
        assert!((t.open - 1.1f64.ln()).abs() < 1e-12);
        assert!(
            log.iter().any(|l| l.contains("history of 1 items")),
            "{log:?}"
        );
        // Written and opened again (the config still says 400/100): once.
        std::fs::write(
            dir.join("market_prices.json"),
            serde_json::to_string(&m.book).unwrap(),
        )
        .unwrap();
        let (m, log) = Market::open(&dir, c);
        assert!((m.book.items["minecraft:diamond"].minutes[0].price - 220.0).abs() < 1e-9);
        assert!(!log.iter().any(|l| l.contains("history")), "{log:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn items_traded_both_ways_get_one_price_at_load() {
        // A config from before 0.29: buy 400, sell 100.
        let mut c = Config::default();
        c.items.insert(
            "minecraft:diamond".into(),
            Price {
                buy: 400,
                sell: 100,
            },
        );
        c.items
            .insert("minecraft:melon_slice".into(), Price { buy: 2, sell: 1 });
        let log = validate(&mut c);
        assert_eq!(
            c.items["minecraft:diamond"],
            Price {
                buy: 200,
                sell: 200
            }
        );
        assert_eq!(c.items["minecraft:melon_slice"], Price { buy: 1, sell: 1 });
        assert!(
            log.iter().any(|l| l
                == "minecraft:diamond: one price now, 200 (it bought for 100 and sold for 400)"),
            "{log:?}"
        );
        // Every item: one price, or one side closed.
        for (id, p) in &c.items {
            assert!(p.buy == p.sell || p.buy == 0 || p.sell == 0, "{id}");
        }
        assert!(validate(&mut c).is_empty(), "once");
    }

    #[test]
    fn the_web_page_gets_prices_moves_and_five_minute_points() {
        let (mut e, _, me, dir) = setup("web");
        let mut m = Market::new(moving());
        let t0 = 1_800_000_000_000i64;
        e.now = t0;
        e.adjust("test_funds", me.uuid, 10_000_000, "").unwrap();
        let mut b = bag(&[], 64 * 36);
        m.tick(t0);
        // An hour of buying a diamond each minute.
        for minute in 1..=60 {
            m.buy(&mut e, &me, &mut b, &["diamond", "1"]);
            let now = t0 + minute * MINUTE_MS;
            e.now = now;
            m.tick(now);
        }
        let now = t0 + 60 * MINUTE_MS;
        let cats = vec![("Mining".to_owned(), vec!["minecraft:diamond".to_owned()])];
        let doc = m.web_view().doc(&cats, "coins", now);
        assert_eq!(doc["schema"], 1);
        let items = doc["items"].as_array().unwrap();
        assert_eq!(items.len(), m.config.items.len());
        let d = items
            .iter()
            .find(|i| i["id"] == "minecraft:diamond")
            .unwrap();
        assert_eq!(d["category"], "Mining");
        assert_eq!(d["volume_24h"], 60);
        assert!(d["change_1h"].as_f64().unwrap() > 0.0, "{d}");
        // One price: what buying and selling both use.
        assert_eq!(d["buy"], d["price"]);
        assert_eq!(d["sell"], d["price"]);
        assert!(d["price"].as_f64().unwrap() > 200.0, "{d}");
        // 60 minute points in 12 five-minute ones (13 when the first hour
        // doesn't start on a 5-minute mark), volumes kept.
        let day = d["day"]["t"].as_array().unwrap();
        assert!((12..=13).contains(&day.len()), "{}", day.len());
        let volume: u64 = d["day"]["volume"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .sum();
        assert_eq!(volume, 60);
        assert!(day.windows(2).all(|w| w[1].as_i64() > w[0].as_i64()));
        let _ = fs::remove_dir_all(dir);
    }
}
