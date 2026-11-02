// The market's moving prices (smp_market's `dynamic`): each item has one
// base price (config.json's `buy`, the same as its `sell` when the market
// trades it both ways), a factor that scales it, and a history. A purchase
// pushes the factor up and a sale down, at once, by an elasticity per item
// (rare, dear items move faster than common ones). A trade of n items costs
// the area under the price along the way it moves the price (`fill`): the
// same curve both ways, so a sale exactly retraces a purchase, and a round
// trip earns nothing, whoever trades and however the price was
// pushed. Trades always fill completely. At `min_factor`..`max_factor` the
// price stops and a trade goes at that bound, the least the market ever pays
// or the most it ever asks. Between trades the factor drifts back toward
// 1 (mean reversion). Items made of one another (`smp_recipes`) share one
// factor, with an elasticity by value, so crafting can't beat the market. The
// history: one point a minute for 24 h, one an hour for 30 days
// (`market_prices.json`), the day's movers and news (`summary`). No game types. Added
// to the game as `game::smp_market_prices`; also a module of this crate, for
// its tests.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MINUTE_MS: i64 = 60_000;
pub const HOUR_MS: i64 = 3_600_000;
/// Points kept: a day of minutes, 30 days of hours.
pub const MINUTES: usize = 1440;
pub const HOURS: usize = 720;

/// `config.json`'s `market.dynamic`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dynamic {
    pub enabled: bool,
    pub min_factor: f64,
    pub max_factor: f64,
    /// How long the factor takes to come halfway back to 1.
    pub half_life_hours: f64,
    /// Unused since 0.29 (trades move the price at once, along one curve):
    /// kept so older configs read the same.
    pub max_trade_impact_percent: f64,
    pub max_player_impact_percent: f64,
    /// Legacy setting, ignored: price movement does not restrict trade volume.
    pub max_minute_move_percent: f64,
    /// Legacy setting: the market allows immediate resale.
    pub round_trip_hours: i64,
    /// Items that move the price 1%, by item (else from its price: dear
    /// items move faster).
    pub depth: BTreeMap<String, f64>,
    /// A day's supply change that makes news, and the least volume for it.
    pub news_percent: f64,
    pub news_min_volume: u64,
}

impl Default for Dynamic {
    fn default() -> Self {
        Self {
            enabled: true,
            min_factor: 0.6,
            max_factor: 1.6,
            half_life_hours: 6.0,
            max_trade_impact_percent: 2.0,
            max_player_impact_percent: 2.0,
            max_minute_move_percent: 5.0,
            round_trip_hours: 24,
            depth: BTreeMap::new(),
            news_percent: 30.0,
            news_min_volume: 64,
        }
    }
}

impl Dynamic {
    /// Its numbers made sane: factors around 1, caps positive.
    pub fn validate(&mut self) {
        let ok = |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        let d = Dynamic::default();
        self.min_factor = ok(self.min_factor, 0.05, 1.0, d.min_factor);
        self.max_factor = ok(self.max_factor, 1.0, 20.0, d.max_factor);
        self.half_life_hours = ok(self.half_life_hours, 0.1, 24.0 * 30.0, d.half_life_hours);
        self.max_trade_impact_percent = ok(
            self.max_trade_impact_percent,
            0.0,
            50.0,
            d.max_trade_impact_percent,
        );
        self.max_player_impact_percent = ok(
            self.max_player_impact_percent,
            0.0,
            50.0,
            d.max_player_impact_percent,
        );
        self.max_minute_move_percent = ok(
            self.max_minute_move_percent,
            0.0,
            50.0,
            d.max_minute_move_percent,
        );
        self.round_trip_hours = self.round_trip_hours.clamp(0, 24 * 30);
        self.depth.retain(|_, v| v.is_finite() && *v > 0.0);
    }
}

/// One point of history: its start (ms), the mid price then, the items
/// traded (both sides) and the items players sold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "(i64, f64, u32, u32)", into = "(i64, f64, u32, u32)")]
pub struct Point {
    pub t: i64,
    pub price: f64,
    pub volume: u32,
    pub sold: u32,
}

impl From<(i64, f64, u32, u32)> for Point {
    fn from((t, price, volume, sold): (i64, f64, u32, u32)) -> Self {
        Self {
            t,
            price,
            volume,
            sold,
        }
    }
}

impl From<Point> for (i64, f64, u32, u32) {
    fn from(p: Point) -> Self {
        (p.t, (p.price * 100.0).round() / 100.0, p.volume, p.sold)
    }
}

/// Which history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Minute,
    Hour,
}

/// The price of one now (`mid`), and what one costs to buy and one brings
/// when sold, in whole coins (the price moves a little as a trade goes, and
/// coins round against the trader).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quote {
    pub mid: f64,
    pub buy: i64,
    pub sell: i64,
}

/// One item's moving price.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub factor: f64,
    pub minutes: Vec<Point>,
    pub hours: Vec<Point>,
    /// The log of the factor when this minute began (the minute's band is
    /// around it).
    pub open: f64,
    /// This minute's volume and sales.
    pub volume: u32,
    pub sold: u32,
    /// The hour being summed.
    pub hour_volume: u32,
    pub hour_sold: u32,
}

/// `market_prices.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Book {
    pub items: BTreeMap<String, Track>,
    /// The start of the minute the flows are for.
    pub minute: i64,
    /// The UTC day the last summary was for.
    pub summary_day: i64,
    /// What each player bought (item, when, how many), for the round-trip
    /// rule.
    pub bought: BTreeMap<String, Vec<(String, i64, u32)>>,
    /// The last daily summary's lines (for the web page and Kira).
    pub last_summary: Vec<String>,
    /// The base price each item's history is quoted in (since 0.29).
    pub bases: BTreeMap<String, f64>,
}

/// What `value` coins of a purchase cost in whole coins (rounded up; a
/// hundredth of a coin forgiven, so a whole price stays whole).
pub fn to_pay(value: f64) -> i64 {
    (value - 0.01).ceil().max(0.0) as i64
}

/// What `value` coins of a sale bring in whole coins (rounded down).
pub fn to_get(value: f64) -> i64 {
    (value + 0.01).floor().max(0.0) as i64
}

/// One price for an item the market both buys and sells at `buy` and `sell`
/// (before 0.29 they differed): their geometric mean, whole, at least 1.
pub fn one_price(buy: i64, sell: i64) -> i64 {
    match (buy > 0, sell > 0) {
        (true, true) => ((buy as f64 * sell as f64).sqrt().round() as i64).max(1),
        (true, false) => buy,
        _ => sell.max(0),
    }
}

/// The base mid price of an item priced `buy` and `sell`.
pub fn base_mid(buy: i64, sell: i64) -> f64 {
    match (buy > 0, sell > 0) {
        (true, true) => (buy + sell) as f64 / 2.0,
        (true, false) => buy as f64,
        _ => sell as f64,
    }
}

impl Book {
    fn track(&mut self, id: &str) -> &mut Track {
        self.items.entry(id.to_owned()).or_insert_with(|| Track {
            factor: 1.0,
            ..Track::default()
        })
    }

    /// The factor now (1 for an item never traded).
    pub fn factor(&self, id: &str) -> f64 {
        self.items
            .get(id)
            .map_or(1.0, |t| if t.factor > 0.0 { t.factor } else { 1.0 })
    }

    /// The price now of an item based at `mid` (one unit moving its log
    /// factor `delta`): one, and what one costs and brings.
    pub fn quote(&self, config: &Dynamic, id: &str, mid: f64, delta: f64) -> Quote {
        Quote {
            mid: mid * self.factor(id),
            buy: to_pay(self.one(config, id, mid, delta, true)),
            sell: to_get(self.one(config, id, mid, delta, false)),
        }
    }

    /// One unit's value now along the curve (within the bounds; the
    /// minute's band aside, so a side the band has closed still shows what
    /// its next one goes for).
    fn one(&self, config: &Dynamic, id: &str, mid: f64, delta: f64, buy: bool) -> f64 {
        let s0 = self.factor(id).ln();
        let s1 = if buy {
            (s0 + delta).min(config.max_factor.ln().max(s0))
        } else {
            (s0 - delta).max(config.min_factor.ln().min(s0))
        };
        if delta <= 0.0 || s1 == s0 {
            return mid * s0.exp();
        }
        mid / delta * (s1.exp() - s0.exp()).abs()
            + (1.0 - (s1 - s0).abs() / delta).max(0.0) * mid * s1.exp()
    }

    /// Items that move `id` (based at `mid`) by 1%.
    pub fn depth(config: &Dynamic, id: &str, mid: f64) -> f64 {
        config
            .depth
            .get(id)
            .copied()
            .unwrap_or_else(|| (400.0 / mid.max(0.01)).clamp(2.0, 400.0))
    }

    /// How far one of `id` (based at `mid`) moves the log of its factor.
    pub fn unit_move(config: &Dynamic, id: &str, mid: f64) -> f64 {
        0.01 / Self::depth(config, id, mid)
    }

    /// What `n` of `id` (based at `mid`, each moving the log factor by
    /// `delta`) are worth bought (`buy`) or sold now: the area under the
    /// price as the trade moves it, exactly, so the same items sold back
    /// retrace it. At a price bound every remaining item trades at the bound.
    /// The value,
    /// the log factor after, and the full number of items traded.
    pub fn fill(
        &self,
        config: &Dynamic,
        id: &str,
        mid: f64,
        delta: f64,
        n: u32,
        buy: bool,
    ) -> (f64, f64, u32) {
        let s0 = self.factor(id).ln();
        let sign = if buy { 1.0 } else { -1.0 };
        let bound = if buy {
            config.max_factor.ln() - s0
        } else {
            s0 - config.min_factor.ln()
        };
        if delta <= 0.0 {
            return (f64::from(n) * mid * s0.exp(), s0, n);
        }
        let done = n;
        let reach = (f64::from(done) * delta).min(bound.max(0.0));
        let s1 = s0 + sign * reach;
        let curve = mid / delta * (s1.exp() - s0.exp()).abs();
        let flat = (f64::from(done) - reach / delta).max(0.0) * mid * s1.exp();
        (curve + flat, s1, done)
    }

    /// A trade of `n` of `id` (based at `mid`, `delta` a unit) now: its
    /// value (`fill`), the factor moved at once for `id` and every item in
    /// `family` (the items it is made of or into), the volume counted.
    #[allow(
        clippy::too_many_arguments,
        reason = "one trade's facts, read in one place"
    )]
    pub fn trade(
        &mut self,
        config: &Dynamic,
        id: &str,
        family: &[String],
        mid: f64,
        delta: f64,
        n: u32,
        buy: bool,
    ) -> f64 {
        let (value, s1, n) = self.fill(config, id, mid, delta, n, buy);
        let factor = s1.exp();
        self.track(id).factor = factor;
        for other in family {
            self.track(other).factor = factor;
        }
        let t = self.track(id);
        t.volume = t.volume.saturating_add(n);
        t.hour_volume = t.hour_volume.saturating_add(n);
        if !buy {
            t.sold = t.sold.saturating_add(n);
            t.hour_sold = t.hour_sold.saturating_add(n);
        }
        value
    }

    /// Items made of one another share one factor: each family's takes the
    /// first member's that has one (after a config adds a family).
    /// At load: each minute's band opens where its price is, and a book
    /// from before 0.29 (no `bases`), whose history is quoted in the old
    /// mid of buy and sell (`old`), gets it scaled to the one price of now
    /// (`bases`), so the charts and the moves see no
    /// cliff. The items it scaled, by how much.
    pub fn settle(
        &mut self,
        old: &BTreeMap<String, f64>,
        bases: &BTreeMap<String, f64>,
    ) -> BTreeMap<String, f64> {
        let mut scaled = BTreeMap::new();
        if self.bases.is_empty() {
            for (id, t) in self.items.iter_mut() {
                let (Some(&was), Some(&now)) = (old.get(id), bases.get(id)) else {
                    continue;
                };
                if was > 0.0 && now > 0.0 && (now / was - 1.0).abs() > 1e-9 {
                    let k = now / was;
                    for p in t.minutes.iter_mut().chain(t.hours.iter_mut()) {
                        p.price *= k;
                    }
                    scaled.insert(id.clone(), k);
                }
            }
        }
        for t in self.items.values_mut() {
            t.open = if t.factor > 0.0 { t.factor.ln() } else { 0.0 };
        }
        self.bases = bases.clone();
        scaled
    }

    pub fn link(&mut self, families: &[Vec<String>]) {
        for f in families {
            let Some((factor, open)) = f
                .iter()
                .find_map(|id| self.items.get(id).map(|t| (t.factor, t.open)))
            else {
                continue;
            };
            for id in f {
                let t = self.track(id);
                t.factor = factor;
                t.open = open;
            }
        }
    }

    /// The minute timer: every minute since the last one, each item's
    /// factor drawn back toward 1 and kept in bounds, the minute's band set,
    /// a point written (and an hour's at each hour). `mids`: the items
    /// traded, with their base mid. Whether a minute passed.
    pub fn tick(&mut self, config: &Dynamic, mids: &BTreeMap<String, f64>, now: i64) -> bool {
        let minute = now.div_euclid(MINUTE_MS) * MINUTE_MS;
        if self.minute == 0 {
            self.minute = minute;
            return false;
        }
        if minute <= self.minute {
            return false;
        }
        // After a long stop, one step for the whole gap (the reversion
        // covers it), its point at the last minute.
        let gap = ((minute - self.minute) / MINUTE_MS).max(1) as f64;
        let decay = 0.5f64.powf(gap / (config.half_life_hours * 60.0));
        let (lo, hi) = (config.min_factor.ln(), config.max_factor.ln());
        let closed = self.minute;
        for (id, mid) in mids {
            let t = self.track(id);
            let log = (t.factor.max(1e-9).ln() * decay).clamp(lo, hi);
            t.factor = log.exp();
            t.open = log;
            let p = Point {
                t: closed,
                price: mid * t.factor,
                volume: std::mem::take(&mut t.volume),
                sold: std::mem::take(&mut t.sold),
            };
            t.minutes.push(p);
            if t.minutes.len() > MINUTES {
                let extra = t.minutes.len() - MINUTES;
                t.minutes.drain(..extra);
            }
            if minute.div_euclid(HOUR_MS) != closed.div_euclid(HOUR_MS) {
                t.hours.push(Point {
                    t: closed.div_euclid(HOUR_MS) * HOUR_MS,
                    price: p.price,
                    volume: std::mem::take(&mut t.hour_volume),
                    sold: std::mem::take(&mut t.hour_sold),
                });
                if t.hours.len() > HOURS {
                    let extra = t.hours.len() - HOURS;
                    t.hours.drain(..extra);
                }
            }
        }
        // Items no longer traded go.
        self.items.retain(|id, _| mids.contains_key(id));
        self.minute = minute;
        // Purchases past the round-trip window go.
        let since = now - config.round_trip_hours * HOUR_MS;
        for v in self.bought.values_mut() {
            v.retain(|(_, t, _)| *t > since);
        }
        self.bought.retain(|_, v| !v.is_empty());
        true
    }

    /// `uuid` bought `n` of `id` at `now`.
    pub fn bought(&mut self, uuid: &str, id: &str, n: u32, now: i64) {
        self.bought
            .entry(uuid.to_owned())
            .or_default()
            .push((id.to_owned(), now, n));
    }

    /// How many of `id` `uuid` bought within the round-trip window.
    pub fn recently_bought(&self, config: &Dynamic, uuid: &str, id: &str, now: i64) -> u32 {
        let since = now - config.round_trip_hours * HOUR_MS;
        self.bought.get(uuid).map_or(0, |v| {
            v.iter()
                .filter(|(i, t, _)| i == id && *t > since)
                .map(|e| e.2)
                .sum()
        })
    }

    pub fn history(&self, id: &str, span: Span) -> &[Point] {
        match self.items.get(id) {
            Some(t) => match span {
                Span::Minute => &t.minutes,
                Span::Hour => &t.hours,
            },
            None => &[],
        }
    }

    /// Items of `id` traded in the last `hours` (minutes for a day, hours
    /// beyond).
    pub fn volume(&self, id: &str, now: i64, hours: u32) -> u64 {
        let Some(t) = self.items.get(id) else {
            return 0;
        };
        let since = now - i64::from(hours) * HOUR_MS;
        let live = u64::from(t.volume);
        if hours <= 24 {
            return live
                + t.minutes
                    .iter()
                    .rev()
                    .take_while(|p| p.t >= since)
                    .map(|p| u64::from(p.volume))
                    .sum::<u64>();
        }
        live + u64::from(t.hour_volume)
            + t.hours
                .iter()
                .rev()
                .take_while(|p| p.t >= since)
                .map(|p| u64::from(p.volume))
                .sum::<u64>()
    }

    /// Items players sold of `id` between `from` and `to` (hours, then
    /// minutes not yet in an hour).
    fn sold_between(&self, id: &str, from: i64, to: i64) -> u64 {
        let Some(t) = self.items.get(id) else {
            return 0;
        };
        let hours: u64 = t
            .hours
            .iter()
            .filter(|p| p.t >= from && p.t < to)
            .map(|p| u64::from(p.sold))
            .sum();
        let last_hour = t.hours.last().map_or(i64::MIN, |p| p.t + HOUR_MS);
        let minutes: u64 = t
            .minutes
            .iter()
            .filter(|p| p.t >= from.max(last_hour) && p.t < to)
            .map(|p| u64::from(p.sold))
            .sum();
        hours + minutes
    }

    /// The change of `id`'s mid over the last `minutes`, in percent (None
    /// without a point that old).
    pub fn change(&self, id: &str, now: i64, minutes: u32, mid_now: f64) -> Option<f64> {
        let t = self.items.get(id)?;
        let at = now - i64::from(minutes) * MINUTE_MS;
        let then = if minutes <= MINUTES as u32 {
            t.minutes.iter().find(|p| p.t >= at)?
        } else {
            t.hours.iter().find(|p| p.t >= at)?
        };
        (then.price > 0.0).then(|| (mid_now / then.price - 1.0) * 100.0)
    }

    /// The day's movers and news, for the chat and Discord: the three
    /// biggest risers and fallers over 24 h (at least 1%), then supply
    /// changes of at least `news_percent` (today's sales against the day
    /// before's, both busy enough). `names`: each item's display name and
    /// mid now.
    pub fn summary(
        &self,
        config: &Dynamic,
        items: &[(String, String, f64)],
        now: i64,
    ) -> Vec<String> {
        let mut moves: Vec<(f64, &str)> = items
            .iter()
            .filter_map(|(id, name, mid)| {
                Some((self.change(id, now, 24 * 60, *mid)?, name.as_str()))
            })
            .filter(|(c, _)| c.abs() >= 1.0)
            .collect();
        moves.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut lines = Vec::new();
        let fmt = |v: &[(f64, &str)]| {
            v.iter()
                .map(|(c, n)| format!("{n} {}", arrow(*c)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let up: Vec<_> = moves
            .iter()
            .filter(|m| m.0 > 0.0)
            .take(3)
            .copied()
            .collect();
        let down: Vec<_> = moves
            .iter()
            .rev()
            .filter(|m| m.0 < 0.0)
            .take(3)
            .copied()
            .collect();
        if !up.is_empty() {
            lines.push(format!("Market today: up {}", fmt(&up)));
        }
        if !down.is_empty() {
            lines.push(format!("Market today: down {}", fmt(&down)));
        }
        let day = 24 * HOUR_MS;
        for (id, name, _) in items {
            let today = self.sold_between(id, now - day, now + 1);
            let before = self.sold_between(id, now - 2 * day, now - day);
            if today.max(before) < config.news_min_volume || before == 0 {
                continue;
            }
            let c = (today as f64 / before as f64 - 1.0) * 100.0;
            if c.abs() >= config.news_percent {
                let how = if c > 0.0 { "up" } else { "down" };
                lines.push(format!(
                    "News: {name} supply {how} {:.0}% today ({today} sold, {before} the day before).",
                    c.abs()
                ));
            }
        }
        lines
    }
}

/// `▲ 2.4%` or `▼ 1.0%`.
pub fn arrow(change: f64) -> String {
    if change >= 0.0 {
        format!("▲ {change:.1}%")
    } else {
        format!("▼ {:.1}%", -change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000_000;

    fn mids() -> BTreeMap<String, f64> {
        [("minecraft:diamond", 250.0), ("minecraft:wheat", 2.0)]
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect()
    }

    fn book() -> Book {
        let mut b = Book::default();
        b.tick(&Dynamic::default(), &mids(), T0);
        b
    }

    const D: &str = "minecraft:diamond";

    /// One diamond's move (based at 250: 0.5%).
    fn delta() -> f64 {
        Book::unit_move(&Dynamic::default(), D, 250.0)
    }

    /// Buys or sells `n` diamonds (as many as the minute allows): the coins
    /// paid (negative) or received, and how many.
    fn deal(b: &mut Book, n: u32, buy: bool) -> (i64, u32) {
        let done = b.fill(&Dynamic::default(), D, 250.0, delta(), n, buy).2;
        let v = b.trade(&Dynamic::default(), D, &[], 250.0, delta(), n, buy);
        (if buy { -to_pay(v) } else { to_get(v) }, done)
    }

    #[test]
    fn purchases_raise_and_sales_lower_the_price_at_once_along_one_curve() {
        let c = Dynamic::default();
        let mut b = book();
        let q = b.quote(&c, D, 250.0, delta());
        // One price; one diamond moves it 0.5%, so one costs and brings the
        // price give or take a quarter of a percent.
        assert_eq!((q.mid, q.buy, q.sell), (250.0, 251, 249));
        assert_eq!(deal(&mut b, 1, false), (249, 1));
        assert!((b.factor(D) - (-0.005f64).exp()).abs() < 1e-12, "at once");
        assert_eq!(deal(&mut b, 1, true), (-250, 1));
        assert!((b.factor(D) - 1.0).abs() < 1e-12, "and back");
        // Wheat is deep: 64 move it far less than one diamond.
        let w = Book::unit_move(&c, "minecraft:wheat", 2.0);
        b.trade(&c, "minecraft:wheat", &[], 2.0, w, 64, false);
        assert!(1.0 - b.factor("minecraft:wheat") < 0.004);
        // Its family moves with it.
        let fam = vec!["minecraft:hay_block".to_string()];
        b.trade(&c, "minecraft:wheat", &fam, 2.0, w, 64, true);
        assert_eq!(b.factor("minecraft:hay_block"), b.factor("minecraft:wheat"));
    }

    #[test]
    fn a_round_trip_earns_nothing_for_1_64_and_past_the_band() {
        // 1, 64, and 2,000 (far past the minute's 5% band: only what fits
        // goes, and the same count comes back).
        for n in [1u32, 64, 2000] {
            for first_buy in [true, false] {
                let mut b = book();
                let (a, done) = deal(&mut b, n, first_buy);
                let (z, back) = deal(&mut b, done, !first_buy);
                assert_eq!(back, done);
                assert!(a + z <= 0, "{n} {first_buy}: {a} {z}");
                // Within the band the curve is retraced exactly: only the
                // rounding of whole coins (at most 2) remains.
                if n <= 8 {
                    assert!(a + z >= -2, "{n} {first_buy}: {a} {z}");
                }
            }
        }
        // Two players: one pumps, the other sells into it: together they
        // lose, whatever the split.
        let mut b = book();
        let paid = -deal(&mut b, 9, true).0;
        let got = deal(&mut b, 9, false).0;
        assert!(got <= paid, "{got} {paid}");
    }

    #[test]
    fn whatever_the_trades_and_the_drift_items_back_where_they_were_cost_money() {
        let mut b = book();
        let mut seed = 7u64;
        let mut rand = |m: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        let (mut coins, mut held, mut now) = (0i64, 0i64, T0);
        for step in 0..5_000 {
            let n = 1 + rand(200) as u32;
            let buy = held < i64::from(n) || (held < 2_000 && rand(2) == 0);
            let (c, n) = deal(&mut b, n, buy);
            coins += c;
            held += if buy { i64::from(n) } else { -i64::from(n) };
            if step % 7 == 0 {
                now += MINUTE_MS * (1 + rand(30) as i64);
                b.tick(&Dynamic::default(), &mids(), now);
            }
        }
        // Back to no items: sold at the end, a minute at a time.
        while held > 0 {
            let (c, n) = deal(&mut b, held as u32, false);
            coins += c;
            held -= i64::from(n);
            now += MINUTE_MS;
            b.tick(&Dynamic::default(), &mids(), now);
        }
        assert!(coins <= 0, "{coins}");
    }

    #[test]
    fn trades_fill_completely_past_the_old_minute_band() {
        let c = Dynamic::default();
        let mut b = book();
        let (v, s1, done) = b.fill(&c, D, 250.0, delta(), 640, false);
        assert_eq!(done, 640);
        assert!((s1.exp() - c.min_factor).abs() < 1e-9);
        assert!(v > 0.0);
        b.trade(&c, D, &[], 250.0, delta(), 640, false);
        assert_eq!(b.fill(&c, D, 250.0, delta(), 640, false).2, 640);
        assert_eq!(b.fill(&c, D, 250.0, delta(), 640, true).2, 640);
    }

    #[test]
    fn the_price_drifts_back_and_stays_in_bounds() {
        let c = Dynamic::default();
        let mut b = book();
        for m in 1..=600 {
            b.trade(&c, D, &[], 250.0, delta(), 640, true);
            // At the bound the rest goes at it: the most it ever asks.
            b.tick(&c, &mids(), T0 + m * MINUTE_MS);
            assert!(b.factor("minecraft:diamond") <= c.max_factor + 1e-9);
        }
        // At the bound (less a minute's drift).
        let top = b.factor("minecraft:diamond");
        assert!((top - c.max_factor).abs() < 0.01, "{top}");
        // Six hours of quiet: halfway back (in the log).
        b.tick(&c, &mids(), T0 + 600 * MINUTE_MS + 6 * HOUR_MS);
        let f = b.factor("minecraft:diamond");
        assert!((f.ln() - top.ln() / 2.0).abs() < 1e-6, "{f}");
    }

    #[test]
    fn volume_and_history() {
        let c = Dynamic::default();
        let mut b = book();
        for m in 1..=12 {
            b.trade(&c, D, &[], 250.0, delta(), 2, true);
            b.tick(&c, &mids(), T0 + m * MINUTE_MS);
        }
        let now = T0 + 12 * MINUTE_MS;
        assert_eq!(b.volume("minecraft:diamond", now, 24), 24);
        assert_eq!(b.history("minecraft:diamond", Span::Minute).len(), 12);
        let h = b.history("minecraft:diamond", Span::Minute);
        assert!(h.windows(2).all(|w| w[1].t == w[0].t + MINUTE_MS));
        // A point written and read back keeps its numbers.
        let json = serde_json::to_string(&b).unwrap();
        let back: Book = serde_json::from_str(&json).unwrap();
        assert_eq!(back.history("minecraft:diamond", Span::Minute).len(), 12);
        assert!((back.factor("minecraft:diamond") - b.factor("minecraft:diamond")).abs() < 1e-12);
    }

    #[test]
    fn hours_are_summed_and_the_day_makes_news() {
        let c = Dynamic::default();
        let mut b = book();
        let mut now = T0;
        // Yesterday 100 wheat an hour; today 200.
        for h in 0..48 {
            let n = if h < 24 { 100 } else { 200 };
            for m in 0..60 {
                if m == 0 {
                    let w = Book::unit_move(&c, "minecraft:wheat", 2.0);
                    b.trade(&c, "minecraft:wheat", &[], 2.0, w, n, false);
                }
                now += MINUTE_MS;
                b.tick(&c, &mids(), now);
            }
        }
        assert!(b.history("minecraft:wheat", Span::Hour).len() >= 47);
        let lines = b.summary(
            &c,
            &[(
                "minecraft:wheat".into(),
                "Wheat".into(),
                2.0 * b.factor("minecraft:wheat"),
            )],
            now,
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("News: Wheat supply up 100% today")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Market today: down Wheat ▼")),
            "{lines:?}"
        );
    }

    #[test]
    fn the_round_trip_window_and_one_price() {
        let c = Dynamic::default();
        let mut b = book();
        b.bought("u", "minecraft:diamond", 5, T0);
        assert_eq!(
            b.recently_bought(&c, "u", "minecraft:diamond", T0 + HOUR_MS),
            5
        );
        b.tick(&c, &mids(), T0 + 25 * HOUR_MS);
        assert_eq!(
            b.recently_bought(&c, "u", "minecraft:diamond", T0 + 25 * HOUR_MS),
            0
        );
        assert!(b.bought.is_empty());
        // One price from the old two: their geometric mean, whole.
        assert_eq!(one_price(400, 100), 200);
        assert_eq!(one_price(30, 8), 15);
        assert_eq!(one_price(2, 1), 1);
        assert_eq!((one_price(5, 0), one_price(0, 1)), (5, 1));
        assert_eq!((to_pay(250.004), to_get(249.996)), (250, 250));
        assert_eq!((to_pay(1.94), to_get(1.94)), (2, 1));
    }
}
