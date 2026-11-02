// The auction house (`/ah`): players sell to players. A listing is a fixed
// price or a timed auction; listing is free, and a sale pays the seller the
// whole price. Listed items leave the seller's inventory at once; what
// doesn't sell (and what an auction's winner gets) waits in the player's mail
// (`/ah claim`, or the menu's mail page). Bids are held in the `auction`
// pool until the auction ends or someone bids more. A bid in the last
// `snipe_seconds` pushes the end back to that much. The seller may be
// offline: their money goes to their account and a note waits for them.
//
// Everything (listings, mail, notes, today's numbers) is written into
// `economy.json` with the balances and pools (`Economy::ext("auction")`), so
// a crash can never split the money from the items. The game runs every
// action on its one server thread, under the SMP's lock: of two players who
// buy the same listing at once, the second finds it gone.
//
// Plain items only for now (not worn, named or enchanted): the hook point
// that gives a whole stack with its components (`native_give_stack`) isn't
// stamped yet. No game types: the glue lends the items (`Bag`) and draws the
// menu from `Cell`s. Added to the game as `game::smp_auction`; also a module
// of this crate, for its tests.

use super::smp_economy::{DAY_MS, Economy, MAX_AMOUNT, Out, Player, parse_amount};
use super::smp_market::{Bag, item_name, valid_id};
use super::smp_shop::{Category, title};
use super::smp_store::{parse_uuid, uuid_string};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The pool bids wait in.
pub const POOL: &str = "auction";
/// Rows of the menu and its slots; listing slots a page.
pub const ROWS: u8 = 6;
pub const SIZE: usize = 54;
pub const PER_PAGE: usize = 45;
/// Sales kept for the reports and `/hub`.
const SALES: usize = 100;

const BACK: usize = 45;
const MINE: usize = 46;
const PREV: usize = 47;
const BALANCE: usize = 48;
const HELP: usize = 49;
const MAIL: usize = 50;
const NEXT: usize = 51;
const CLOSE: usize = 53;
const CONFIRM: usize = 20;
const PREVIEW: usize = 22;
const CANCEL: usize = 24;
const CLAIM_ALL: usize = 49;
/// The home page's category buttons (the first is "All").
const CATEGORY_SLOTS: [usize; 21] = [
    10, 11, 12, 13, 14, 15, 16, 19, 20, 21, 22, 23, 24, 25, 28, 29, 30, 31, 32, 33, 34,
];

/// `config.json`'s `auction`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    /// Legacy setting, ignored: players may have any number of listings.
    pub max_listings: usize,
    /// How long a fixed-price listing stays.
    pub listing_hours: i64,
    /// An auction's length when not given, and the longest.
    pub auction_hours: i64,
    pub auction_hours_max: i64,
    /// The smallest raise, in percent of the current bid (at least 1 coin).
    pub bid_step_percent: i64,
    /// A bid this close to the end pushes the end back to this much.
    pub snipe_seconds: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            max_listings: 0,
            listing_hours: 48,
            auction_hours: 12,
            auction_hours_max: 24,
            bid_step_percent: 5,
            snipe_seconds: 60,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bid {
    pub uuid: String,
    pub name: String,
    pub amount: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub id: u64,
    pub seller: String,
    pub seller_name: String,
    pub item: String,
    pub count: u32,
    /// The price (fixed), or the starting bid (auction).
    pub price: i64,
    pub auction: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bid: Option<Bid>,
    pub created: i64,
    pub ends: i64,
}

impl Listing {
    /// What it costs now: the price, the bid, or the starting bid.
    pub fn amount(&self) -> i64 {
        self.bid.as_ref().map_or(self.price, |b| b.amount)
    }
}

/// Items waiting for a player.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Parcel {
    pub item: String,
    pub count: u32,
    /// `expired`, `won`, `cancelled`...
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sale {
    pub time: i64,
    pub item: String,
    pub count: u32,
    pub price: i64,
    pub auction: bool,
}

/// Today's numbers (UTC day).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Today {
    pub day: i64,
    pub sales: u32,
    pub volume: i64,
}

/// What `economy.json`'s `ext.auction` holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Data {
    pub next_id: u64,
    pub listings: Vec<Listing>,
    pub mail: BTreeMap<String, Vec<Parcel>>,
    /// Lines for players who were offline, given when they are next on.
    pub notes: BTreeMap<String, Vec<String>>,
    pub sales: Vec<Sale>,
    pub today: Today,
}

/// Which listings a list page shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    All,
    /// By index in the categories as shown.
    Category(usize),
    Search(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Page {
    Home,
    List(Filter, usize),
    Mine,
    Mail,
    /// A listing about to be bought or bid on, and the page to go back to.
    Confirm(u64, Box<Page>),
}

/// One slot of the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Filler,
    /// Icon, name, listings in it.
    Category(String, String, usize),
    Listing(u64),
    /// A parcel of the player's mail, by index.
    Parcel(usize),
    Back,
    Prev,
    Next,
    Mine(usize),
    Mail(usize),
    Balance,
    Help,
    Close,
    /// Buy or bid on this listing.
    Confirm(u64),
    Cancel,
    ClaimAll,
}

/// What a click asks the glue for.
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    Nothing,
    /// The page changed: draw it again.
    Redraw,
    Close,
    Buy(u64),
    Bid(u64),
    Withdraw(u64),
    Claim(usize),
    ClaimAll,
}

/// What a command asks for: lines, or the menu on a page.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Out(Vec<Out>),
    Open(Page),
}

pub const HELP_TEXT: &str = "Click a listing to buy or bid · /ah sell <price> [count] · /ah auction <start> [hours] · /ah claim";

pub struct AuctionHouse {
    pub config: Config,
    pub data: Data,
    dirty: bool,
    /// The open menus: window and page.
    pub views: HashMap<u128, (Option<i32>, Page)>,
    /// The players on (the glue sets it once a second): a line to one who
    /// isn't waits as a note.
    pub online: HashSet<u128>,
    /// Money moves that failed after the items moved (the audit log
    /// couldn't be written): the glue logs them.
    pub problems: Vec<String>,
}

/// `2d 5h`, `3h 05m`, `12m`: the time left on a listing.
fn span(ms: i64) -> String {
    let m = ms.max(0) / 60_000;
    let (d, h, m) = (m / 1440, m / 60 % 24, m % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// `price * percent / 100`, without overflow.
fn percent(price: i64, percent: i64) -> i64 {
    (i128::from(price) * i128::from(percent.clamp(0, 100)) / 100) as i64
}

/// The category of `item` among `cats` (an index), or `cats.len()` ("Other").
pub fn category_of(item: &str, cats: &[Category]) -> usize {
    if let Some(i) = cats.iter().position(|c| c.items.iter().any(|i| i == item)) {
        return i;
    }
    let path = item.rsplit(':').next().unwrap_or(item);
    let by = |words: &[&str]| {
        words
            .iter()
            .any(|w| path.ends_with(w) || path.starts_with(w))
    };
    let named = |name: &str| cats.iter().position(|c| c.name == name);
    let guess = if by(&[
        "_ore",
        "raw_",
        "_ingot",
        "_nugget",
        "diamond",
        "emerald",
        "netherite_scrap",
    ]) {
        named("Mining")
    } else if by(&["_log", "_planks", "_wood", "_sapling"]) {
        named("Wood")
    } else if by(&["_seeds", "_slice", "carrot", "potato", "beetroot"]) {
        named("Farming")
    } else if by(&[
        "_sword",
        "_pickaxe",
        "_axe",
        "_shovel",
        "_hoe",
        "_helmet",
        "_chestplate",
        "_leggings",
        "_boots",
        "bow",
        "shield",
        "trident",
        "fishing_rod",
        "shears",
        "flint_and_steel",
    ]) {
        named("Tools")
    } else {
        None
    };
    guess.unwrap_or(cats.len())
}

impl AuctionHouse {
    /// The auction house as the economy keeps it.
    pub fn open(economy: &Economy, config: Config) -> Self {
        let data = economy
            .ext("auction")
            .and_then(|v| serde_json::from_value::<Data>(v.clone()).ok())
            .unwrap_or_default();
        Self {
            config,
            data,
            dirty: false,
            views: HashMap::new(),
            online: HashSet::new(),
            problems: Vec::new(),
        }
    }

    /// Writes the state into the economy (saved with the money).
    pub fn store(&mut self, economy: &mut Economy) {
        if self.dirty {
            economy.set_ext(
                "auction",
                serde_json::to_value(&self.data).unwrap_or_default(),
            );
            self.dirty = false;
        }
    }

    pub fn listing(&self, id: u64) -> Option<&Listing> {
        self.data.listings.iter().find(|l| l.id == id)
    }

    fn mail_of(&self, uuid: u128) -> &[Parcel] {
        self.data
            .mail
            .get(&uuid_string(uuid))
            .map_or(&[], Vec::as_slice)
    }

    /// Items to `uuid`'s mail.
    fn post(&mut self, uuid: &str, item: &str, count: u32, why: &str) {
        if count == 0 {
            return;
        }
        self.data
            .mail
            .entry(uuid.to_owned())
            .or_default()
            .push(Parcel {
                item: item.to_owned(),
                count,
                why: why.to_owned(),
            });
        self.dirty = true;
    }

    /// A line to a player: now if they are on, else when they next are.
    fn notify(&mut self, out: &mut Vec<Out>, uuid: &str, line: String) {
        match parse_uuid(uuid) {
            Some(u) if self.online.contains(&u) => out.push(Out::Tell(u, line)),
            _ => {
                let notes = self.data.notes.entry(uuid.to_owned()).or_default();
                notes.push(line);
                if notes.len() > 20 {
                    notes.remove(0);
                }
                self.dirty = true;
            }
        }
    }

    fn count_today(&mut self, now: i64) -> &mut Today {
        let day = now.div_euclid(DAY_MS);
        if self.data.today.day != day {
            self.data.today = Today {
                day,
                ..Today::default()
            };
        }
        &mut self.data.today
    }

    fn record_sale(&mut self, l: &Listing, price: i64, now: i64) {
        let t = self.count_today(now);
        t.sales += 1;
        t.volume = t.volume.saturating_add(price);
        self.data.sales.push(Sale {
            time: now,
            item: l.item.clone(),
            count: l.count,
            price,
            auction: l.auction,
        });
        if self.data.sales.len() > SALES {
            self.data.sales.remove(0);
        }
    }

    /// `64 wheat`.
    fn what(l: &Listing) -> String {
        format!("{} {}", l.count, item_name(&l.item))
    }

    /// Pays the seller `price` (from the buyer, or from the bids' pool).
    fn pay_seller(
        economy: &mut Economy,
        from: Result<u128, ()>,
        seller: u128,
        price: i64,
    ) -> Result<(), String> {
        match from {
            Ok(buyer) => economy.transfer("ah_sale", buyer, seller, price),
            Err(()) => economy.from_pool("ah_sale", POOL, seller, price, ""),
        }
    }

    /// Runs `/ah ...`; None for another command. `bag`: the acting player's
    /// items; `trades`: survival or adventure.
    #[allow(clippy::too_many_arguments)]
    pub fn command(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        trades: bool,
        bag: &mut dyn Bag,
        now: i64,
        line: &str,
    ) -> Option<Reply> {
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.first() != Some(&"ah") {
            return None;
        }
        if !self.config.enabled {
            return Some(Reply::Out(vec![Out::Error(
                "The auction house is closed.".into(),
            )]));
        }
        let out = |o: Vec<Out>| Some(Reply::Out(o));
        let err = |s: &str| Some(Reply::Out(vec![Out::Error(s.into())]));
        let moves = matches!(words.get(1), Some(&("sell" | "auction" | "bid" | "buy")));
        if moves && !trades {
            return err(
                "The auction house trades only with players in survival or adventure mode.",
            );
        }
        match words[1..] {
            [] => Some(Reply::Open(Page::Home)),
            ["mine"] => Some(Reply::Open(Page::Mine)),
            ["mail"] => Some(Reply::Open(Page::Mail)),
            ["search", ref q @ ..] if !q.is_empty() => Some(Reply::Open(Page::List(
                Filter::Search(q.join(" ").to_ascii_lowercase()),
                0,
            ))),
            ["sell", price] => out(self.list(economy, me, bag, price, None, None, now)),
            ["sell", price, count] => match count.parse::<u32>() {
                Ok(c) => out(self.list(economy, me, bag, price, Some(c), None, now)),
                Err(_) => err("Usage: /ah sell <price> [count]"),
            },
            ["auction", start] => out(self.list(
                economy,
                me,
                bag,
                start,
                None,
                Some(self.config.auction_hours),
                now,
            )),
            ["auction", start, hours] => match hours.parse::<i64>() {
                Ok(h) if (1..=self.config.auction_hours_max).contains(&h) => {
                    out(self.list(economy, me, bag, start, None, Some(h), now))
                }
                _ => err(&format!(
                    "An auction lasts 1 to {} hours.",
                    self.config.auction_hours_max
                )),
            },
            ["bid", id, amount] => match (id.parse::<u64>(), parse_amount(amount)) {
                (Ok(id), Some(a)) => out(self.bid(economy, me, id, Some(a), now)),
                _ => err("Usage: /ah bid <listing> <amount>"),
            },
            ["buy", id] => match id.parse::<u64>() {
                Ok(id) => out(self.buy(economy, me, bag, id, now)),
                Err(_) => err("Usage: /ah buy <listing>"),
            },
            ["cancel", id] => match id.parse::<u64>() {
                Ok(id) => {
                    let mut o = self.withdraw(me, id);
                    if !matches!(o.first(), Some(Out::Error(_))) {
                        o.extend(self.claim(me.uuid, bag, None));
                    }
                    out(o)
                }
                Err(_) => err("Usage: /ah cancel <listing>"),
            },
            ["claim"] => out(self.claim(me.uuid, bag, None)),
            _ => err(
                "Usage: /ah, /ah search <words>, /ah sell <price> [count], /ah auction <start> [hours], /ah bid <listing> <amount>, /ah buy <listing>, /ah cancel <listing>, /ah claim, /ah mine",
            ),
        }
    }

    /// Lists the stack in the player's hand (or `count` of it): a fixed
    /// price, or an auction of `hours` starting at `price`.
    #[allow(clippy::too_many_arguments)]
    fn list(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        bag: &mut dyn Bag,
        price: &str,
        count: Option<u32>,
        hours: Option<i64>,
        now: i64,
    ) -> Vec<Out> {
        let err = |s: String| vec![Out::Error(s)];
        let Some(price) = parse_amount(price) else {
            return err("The price must be a positive whole number.".into());
        };
        let mine = uuid_string(me.uuid);
        let Some((item, held, plain)) = bag.held() else {
            return err("Hold the items you want to sell.".into());
        };
        if !plain || !valid_id(&item) {
            return err(
                "The auction house takes only plain items for now (not worn, named or enchanted)."
                    .into(),
            );
        }
        let count = count.unwrap_or(held);
        if count == 0 || count > held {
            return err(format!("You hold {held} {}.", item_name(&item)));
        }
        let taken = bag.take(&item, count);
        if taken != count {
            bag.give(&item, taken);
            return err("Those items couldn't be taken.".into());
        }
        self.data.next_id += 1;
        let id = self.data.next_id;
        let ends = now + hours.unwrap_or(self.config.listing_hours).max(1) * 3_600_000;
        let entry = serde_json::json!({
            "kind": "ah_list", "id": id, "seller": mine, "item": &item, "count": count,
            "price": price, "auction": hours.is_some(), "ends": ends,
        });
        if let Err(e) = economy.record(entry) {
            bag.give(&item, taken);
            return err(e);
        }
        let l = Listing {
            id,
            seller: mine,
            seller_name: me.name.clone(),
            item,
            count,
            price,
            auction: hours.is_some(),
            bid: None,
            created: now,
            ends,
        };
        let line = match hours {
            None => format!(
                "Listed #{id}: {} for {} ({:.2} each), {}h.",
                Self::what(&l),
                economy.money(price),
                price as f64 / f64::from(count),
                self.config.listing_hours
            ),
            Some(h) => format!(
                "Auction #{id}: {} from {}, ends in {h}h.",
                Self::what(&l),
                economy.money(price)
            ),
        };
        self.data.listings.push(l);
        self.dirty = true;
        vec![Out::Reply(line)]
    }

    /// The smallest bid `l` takes now.
    pub fn min_bid(&self, l: &Listing) -> i64 {
        match &l.bid {
            None => l.price,
            Some(b) => b
                .amount
                .saturating_add(percent(b.amount, self.config.bid_step_percent).max(1)),
        }
    }

    /// Buys a fixed-price listing whole.
    pub fn buy(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        bag: &mut dyn Bag,
        id: u64,
        now: i64,
    ) -> Vec<Out> {
        let err = |s: String| vec![Out::Error(s)];
        let Some(i) = self
            .data
            .listings
            .iter()
            .position(|l| l.id == id && l.ends > now)
        else {
            return err("That listing is gone (sold, withdrawn or ended).".into());
        };
        let l = self.data.listings[i].clone();
        if l.auction {
            return err(format!("#{id} is an auction: bid on it."));
        }
        if l.seller == uuid_string(me.uuid) {
            return err("That's your own listing (/ah cancel to take it back).".into());
        }
        if economy.balance(me.uuid) < l.price {
            return err(format!(
                "You only have {}.",
                economy.money(economy.balance(me.uuid))
            ));
        }
        let Some(seller) = parse_uuid(&l.seller) else {
            return err("That listing is damaged.".into());
        };
        if bag.room(&l.item) < l.count {
            return err(format!("Make room for {} first.", Self::what(&l)));
        }
        let entry = serde_json::json!({
            "kind": "ah_buy", "id": id, "buyer": uuid_string(me.uuid), "seller": &l.seller,
            "item": &l.item, "count": l.count, "price": l.price,
        });
        if let Err(e) = economy.record(entry) {
            return err(e);
        }
        // The listing leaves first: nothing can sell it twice.
        self.data.listings.remove(i);
        self.dirty = true;
        let given = bag.give(&l.item, l.count);
        if given == 0 {
            self.data.listings.insert(i, l);
            return err("The items couldn't be given: nothing was bought.".into());
        }
        self.post(&uuid_string(me.uuid), &l.item, l.count - given, "bought");
        if let Err(e) = Self::pay_seller(economy, Ok(me.uuid), seller, l.price) {
            self.problems.push(format!("auction house sale #{id}: {e}"));
        }
        self.record_sale(&l, l.price, now);
        let mut out = vec![Out::Reply(format!(
            "Bought {} from {} for {}.{}",
            Self::what(&l),
            l.seller_name,
            economy.money(l.price),
            if given < l.count {
                " What didn't fit is in your mail: /ah claim."
            } else {
                ""
            }
        ))];
        let line = format!(
            "{} bought your {} for {}.",
            me.name,
            Self::what(&l),
            economy.money(l.price)
        );
        self.notify(&mut out, &l.seller, line);
        out
    }

    /// Bids `amount` (None: the smallest bid it takes) on an auction.
    pub fn bid(
        &mut self,
        economy: &mut Economy,
        me: &Player,
        id: u64,
        amount: Option<i64>,
        now: i64,
    ) -> Vec<Out> {
        let err = |s: String| vec![Out::Error(s)];
        let Some(i) = self
            .data
            .listings
            .iter()
            .position(|l| l.id == id && l.ends > now)
        else {
            return err("That auction is over.".into());
        };
        let l = self.data.listings[i].clone();
        if !l.auction {
            return err(format!("#{id} has a fixed price: buy it."));
        }
        let mine = uuid_string(me.uuid);
        if l.seller == mine {
            return err("You can't bid on your own auction.".into());
        }
        let min = self.min_bid(&l);
        let amount = amount.unwrap_or(min);
        if amount < min || amount > MAX_AMOUNT {
            return err(format!("The smallest bid now is {}.", economy.money(min)));
        }
        let mut out = Vec::new();
        match &l.bid {
            // Raising one's own bid: only the difference is held.
            Some(b) if b.uuid == mine => {
                if let Err(e) = economy.to_pool("ah_bid", me.uuid, POOL, amount - b.amount, "") {
                    return err(e);
                }
            }
            prev => {
                if let Err(e) = economy.to_pool("ah_bid", me.uuid, POOL, amount, "") {
                    return err(e);
                }
                if let Some(b) = prev
                    && let Some(u) = parse_uuid(&b.uuid)
                {
                    if let Err(e) = economy.from_pool("ah_bid_refund", POOL, u, b.amount, "") {
                        self.problems.push(format!("auction #{id} refund: {e}"));
                    }
                    let line = format!(
                        "You were outbid on #{id} ({}): {} back. The bid is {}.",
                        Self::what(&l),
                        economy.money(b.amount),
                        economy.money(amount)
                    );
                    self.notify(&mut out, &b.uuid.clone(), line);
                }
            }
        }
        let snipe = self.config.snipe_seconds.max(0) * 1000;
        let l = &mut self.data.listings[i];
        l.bid = Some(Bid {
            uuid: mine,
            name: me.name.clone(),
            amount,
        });
        let extended = l.ends - now < snipe;
        if extended {
            l.ends = now + snipe;
        }
        let l = l.clone();
        self.dirty = true;
        out.insert(
            0,
            Out::Reply(format!(
                "You bid {} on #{id} ({}), ends in {}.{}",
                economy.money(amount),
                Self::what(&l),
                span(l.ends - now),
                if extended {
                    " A late bid: the end moved back."
                } else {
                    ""
                }
            )),
        );
        out
    }

    /// Takes back one's own listing (an operator of level 3: anyone's) while
    /// no bid holds it; the items go to the seller's mail.
    pub fn withdraw(&mut self, me: &Player, id: u64) -> Vec<Out> {
        let Some(i) = self.data.listings.iter().position(|l| l.id == id) else {
            return vec![Out::Error(format!("No listing #{id}."))];
        };
        let l = &self.data.listings[i];
        if l.seller != uuid_string(me.uuid) && me.level < 3 {
            return vec![Out::Error("That isn't your listing.".into())];
        }
        if l.bid.is_some() {
            return vec![Out::Error("Someone bid on it: it runs to its end.".into())];
        }
        let l = self.data.listings.remove(i);
        self.post(&l.seller, &l.item, l.count, "withdrawn");
        vec![Out::Reply(format!("Took #{id} ({}) back.", Self::what(&l)))]
    }

    /// Gives `uuid` their mail (one parcel, or all of it), as much as fits;
    /// the rest stays.
    pub fn claim(&mut self, uuid: u128, bag: &mut dyn Bag, only: Option<usize>) -> Vec<Out> {
        let key = uuid_string(uuid);
        let Some(mut parcels) = self.data.mail.remove(&key) else {
            return vec![Out::Reply("Your mail is empty.".into())];
        };
        let mut given_all = Vec::new();
        let mut full = false;
        for (i, p) in parcels.iter_mut().enumerate() {
            if only.is_some_and(|o| o != i) {
                continue;
            }
            let n = p.count.min(bag.room(&p.item));
            let given = if n > 0 { bag.give(&p.item, n) } else { 0 };
            p.count -= given.min(p.count);
            full |= p.count > 0;
            if given > 0 {
                given_all.push(format!("{given} {}", item_name(&p.item)));
            }
        }
        parcels.retain(|p| p.count > 0);
        if !parcels.is_empty() {
            self.data.mail.insert(key, parcels);
        }
        self.dirty = true;
        let mut out = Vec::new();
        if !given_all.is_empty() {
            out.push(Out::Reply(format!(
                "From your mail: {}.",
                given_all.join(", ")
            )));
        }
        if full {
            out.push(Out::Error(
                "Your inventory is full: the rest waits in your mail.".into(),
            ));
        }
        if out.is_empty() {
            out.push(Out::Reply("Nothing to claim.".into()));
        }
        out
    }

    /// Once a second: notes to the players who came on, and the listings
    /// that ended (fixed ones back to their seller, auctions to their
    /// winner). Only a comparison while nothing ended.
    pub fn tick(&mut self, economy: &mut Economy, online: &[u128], now: i64) -> Vec<Out> {
        self.online = online.iter().copied().collect();
        let mut out = Vec::new();
        if !self.data.notes.is_empty() {
            for &u in online {
                if let Some(notes) = self.data.notes.remove(&uuid_string(u)) {
                    out.extend(notes.into_iter().map(|n| Out::Tell(u, n)));
                    self.dirty = true;
                }
            }
        }
        if !self.data.listings.iter().any(|l| l.ends <= now) {
            return out;
        }
        let (ended, open): (Vec<Listing>, Vec<Listing>) = std::mem::take(&mut self.data.listings)
            .into_iter()
            .partition(|l| l.ends <= now);
        self.data.listings = open;
        self.dirty = true;
        for l in ended {
            let Some(b) = l.bid.clone() else {
                let why = if l.auction { "no bids" } else { "expired" };
                self.post(&l.seller, &l.item, l.count, why);
                let line = format!(
                    "#{} ({}) ended with {why}: the items are in your mail (/ah claim).",
                    l.id,
                    Self::what(&l)
                );
                self.notify(&mut out, &l.seller.clone(), line);
                continue;
            };
            let seller = parse_uuid(&l.seller).unwrap_or_default();
            if let Err(e) = Self::pay_seller(economy, Err(()), seller, b.amount) {
                self.problems
                    .push(format!("auction #{} payment: {e}", l.id));
            }
            self.post(&b.uuid, &l.item, l.count, "won");
            self.record_sale(&l, b.amount, now);
            let line = format!(
                "You won #{} ({}) for {}: the items are in your mail (/ah claim).",
                l.id,
                Self::what(&l),
                economy.money(b.amount)
            );
            self.notify(&mut out, &b.uuid, line);
            let line = format!(
                "Auction #{} ({}) sold to {} for {}.",
                l.id,
                Self::what(&l),
                b.name,
                economy.money(b.amount)
            );
            self.notify(&mut out, &l.seller.clone(), line);
        }
        out
    }

    /// The listings a filter shows, cheapest a unit first.
    pub fn filtered(&self, filter: &Filter, cats: &[Category], now: i64) -> Vec<&Listing> {
        let mut v: Vec<&Listing> = self
            .data
            .listings
            .iter()
            .filter(|l| l.ends > now)
            .filter(|l| match filter {
                Filter::All => true,
                Filter::Category(c) => category_of(&l.item, cats) == *c,
                Filter::Search(q) => {
                    item_name(&l.item).contains(q.as_str()) || l.item.contains(q.as_str())
                }
            })
            .collect();
        v.sort_by(|a, b| {
            let (pa, pb) = (i128::from(a.amount()), i128::from(b.amount()));
            (pa * i128::from(b.count))
                .cmp(&(pb * i128::from(a.count)))
                .then(a.id.cmp(&b.id))
        });
        v
    }

    /// The categories as the home page shows them: the shop's, then "Other".
    fn categories(cats: &[Category]) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = cats
            .iter()
            .map(|c| (c.icon.clone(), c.name.clone()))
            .collect();
        v.push(("minecraft:chest".into(), "Other".into()));
        v
    }

    /// The page `uuid` sees, as cells.
    pub fn layout(&self, uuid: u128, cats: &[Category], now: i64) -> Vec<Cell> {
        let page = self.views.get(&uuid).map_or(Page::Home, |v| v.1.clone());
        let mut cells = vec![Cell::Empty; SIZE];
        for c in &mut cells[PER_PAGE..] {
            *c = Cell::Filler;
        }
        cells[BALANCE] = Cell::Balance;
        cells[CLOSE] = Cell::Close;
        let mine = uuid_string(uuid);
        match &page {
            Page::Home => {
                let live: Vec<&Listing> =
                    self.data.listings.iter().filter(|l| l.ends > now).collect();
                let mut slots = CATEGORY_SLOTS.iter();
                if let Some(s) = slots.next() {
                    cells[*s] = Cell::Category(
                        "minecraft:nether_star".into(),
                        "All listings".into(),
                        live.len(),
                    );
                }
                for (i, (icon, name)) in Self::categories(cats).into_iter().enumerate() {
                    let n = live
                        .iter()
                        .filter(|l| category_of(&l.item, cats) == i)
                        .count();
                    if let Some(s) = slots.next() {
                        cells[*s] = Cell::Category(icon, name, n);
                    }
                }
                cells[HELP] = Cell::Help;
                cells[MINE] = Cell::Mine(
                    self.data
                        .listings
                        .iter()
                        .filter(|l| l.seller == mine)
                        .count(),
                );
                cells[MAIL] = Cell::Mail(self.mail_of(uuid).len());
            }
            Page::List(filter, p) => {
                let list = self.filtered(filter, cats, now);
                for (slot, l) in list.iter().skip(p * PER_PAGE).take(PER_PAGE).enumerate() {
                    cells[slot] = Cell::Listing(l.id);
                }
                cells[BACK] = Cell::Back;
                cells[HELP] = Cell::Help;
                if *p > 0 {
                    cells[PREV] = Cell::Prev;
                }
                if (p + 1) * PER_PAGE < list.len() {
                    cells[NEXT] = Cell::Next;
                }
            }
            Page::Mine => {
                for (slot, l) in self
                    .data
                    .listings
                    .iter()
                    .filter(|l| l.seller == mine)
                    .take(PER_PAGE)
                    .enumerate()
                {
                    cells[slot] = Cell::Listing(l.id);
                }
                cells[BACK] = Cell::Back;
            }
            Page::Mail => {
                let n = self.mail_of(uuid).len().min(PER_PAGE);
                for (i, c) in cells.iter_mut().take(n).enumerate() {
                    *c = Cell::Parcel(i);
                }
                cells[BACK] = Cell::Back;
                cells[CLAIM_ALL] = Cell::ClaimAll;
            }
            Page::Confirm(id, _) => {
                if self.listing(*id).is_some() {
                    cells[CONFIRM] = Cell::Confirm(*id);
                    cells[PREVIEW] = Cell::Listing(*id);
                    cells[CANCEL] = Cell::Cancel;
                }
                cells[BACK] = Cell::Back;
            }
        }
        cells
    }

    /// What a click on `slot` of `uuid`'s menu means; a new page is kept.
    pub fn click(&mut self, uuid: u128, cats: &[Category], slot: usize, now: i64) -> Click {
        let cells = self.layout(uuid, cats, now);
        let Some(view) = self.views.get_mut(&uuid) else {
            return Click::Nothing;
        };
        let page = view.1.clone();
        let back = |p: &Page| match p {
            Page::Confirm(_, back) => (**back).clone(),
            _ => Page::Home,
        };
        let show = |view: &mut (Option<i32>, Page), p: Page| {
            view.1 = p;
            Click::Redraw
        };
        match (cells.get(slot), &page) {
            (Some(Cell::Close), _) => Click::Close,
            (Some(Cell::Back | Cell::Cancel), p) => show(view, back(p)),
            (Some(Cell::Category(..)), _) => {
                let i = CATEGORY_SLOTS.iter().position(|s| *s == slot).unwrap_or(0);
                let filter = if i == 0 {
                    Filter::All
                } else {
                    Filter::Category(i - 1)
                };
                show(view, Page::List(filter, 0))
            }
            (Some(Cell::Mine(_)), _) => show(view, Page::Mine),
            (Some(Cell::Mail(_)), _) => show(view, Page::Mail),
            (Some(Cell::Prev), Page::List(f, p)) => {
                show(view, Page::List(f.clone(), p.saturating_sub(1)))
            }
            (Some(Cell::Next), Page::List(f, p)) => show(view, Page::List(f.clone(), p + 1)),
            (Some(Cell::Listing(id)), Page::Mine) => Click::Withdraw(*id),
            (Some(Cell::Listing(id)), Page::List(..)) => {
                show(view, Page::Confirm(*id, Box::new(page.clone())))
            }
            (Some(Cell::Confirm(id)), _) => match self.listing(*id) {
                Some(l) if l.auction => Click::Bid(*id),
                Some(_) => Click::Buy(*id),
                None => Click::Redraw,
            },
            (Some(Cell::Parcel(i)), _) => Click::Claim(*i),
            (Some(Cell::ClaimAll), Page::Mail) => Click::ClaimAll,
            _ => Click::Nothing,
        }
    }

    /// A listing's name on its button.
    pub fn label(&self, l: &Listing, now: i64, money: &dyn Fn(i64) -> String) -> String {
        let each = l.amount() as f64 / f64::from(l.count.max(1));
        let left = span(l.ends - now);
        let head = format!("#{} {} {}", l.id, l.count, title(&l.item));
        if l.auction {
            let bid = match &l.bid {
                Some(b) => format!("bid {} by {}", money(b.amount), b.name),
                None => format!("from {}", money(l.price)),
            };
            format!(
                "{head} · auction, {bid} ({each:.2} each) · {} · {left} left",
                l.seller_name
            )
        } else {
            format!(
                "{head} · {} ({each:.2} each) · {} · {left} left",
                money(l.price),
                l.seller_name
            )
        }
    }

    /// The confirm button's words.
    pub fn confirm_label(&self, l: &Listing, money: &dyn Fn(i64) -> String) -> String {
        if l.auction {
            format!(
                "Bid {} (more: /ah bid {} <amount>)",
                money(self.min_bid(l)),
                l.id
            )
        } else {
            format!("Buy {} {} for {}", l.count, title(&l.item), money(l.price))
        }
    }

    /// A parcel of `uuid`'s mail, by index.
    pub fn parcel(&self, uuid: u128, i: usize) -> Option<&Parcel> {
        self.mail_of(uuid).get(i)
    }

    /// The numbers for `/eco stats`, the report and `/hub`.
    #[allow(dead_code)] // the economy report (next)
    pub fn summary(&self, economy: &Economy, now: i64) -> serde_json::Value {
        let today = if self.data.today.day == now.div_euclid(DAY_MS) {
            self.data.today.clone()
        } else {
            Today::default()
        };
        serde_json::json!({
            "listings": self.data.listings.len(),
            "bids_held": economy.pool(POOL),
            "today": today,
            "recent_sales": self.data.sales.iter().rev().take(20).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smp_economy::Config as EcoConfig;
    use std::fs;

    const ALEX: u128 = 1;
    const STEVE: u128 = 2;
    const NOTCH: u128 = 3;
    const T0: i64 = 1_793_577_600_000;

    /// A player's inventory: plain stacks by id, 36 slots of 64.
    #[derive(Default)]
    struct Inv {
        hand: Option<(String, u32)>,
        items: BTreeMap<String, u32>,
        slots_free: u32,
        fail_give: bool,
    }

    impl Bag for Inv {
        fn held(&self) -> Option<(String, u32, bool)> {
            self.hand
                .clone()
                .filter(|h| h.1 > 0)
                .map(|(i, n)| (i, n, true))
        }
        fn carried(&self) -> Vec<(String, u32)> {
            self.items.iter().map(|(k, v)| (k.clone(), *v)).collect()
        }
        fn count(&mut self, item: &str) -> u32 {
            self.items.get(item).copied().unwrap_or(0)
        }
        fn take(&mut self, item: &str, count: u32) -> u32 {
            let have = self.items.entry(item.into()).or_default();
            let n = count.min(*have);
            *have -= n;
            if let Some((i, h)) = &mut self.hand
                && i == item
            {
                *h -= n.min(*h);
            }
            n
        }
        fn room(&mut self, _item: &str) -> u32 {
            self.slots_free * 64
        }
        fn give(&mut self, item: &str, count: u32) -> u32 {
            if self.fail_give {
                return 0;
            }
            let n = count.min(self.slots_free * 64);
            *self.items.entry(item.into()).or_default() += n;
            n
        }
    }

    fn holding(item: &str, n: u32) -> Inv {
        Inv {
            hand: Some((format!("minecraft:{item}"), n)),
            items: BTreeMap::from([(format!("minecraft:{item}"), n)]),
            slots_free: 30,
            fail_give: false,
        }
    }

    fn empty() -> Inv {
        Inv {
            slots_free: 30,
            ..Inv::default()
        }
    }

    fn p(uuid: u128, name: &str) -> Player {
        Player {
            uuid,
            name: name.into(),
            level: 0,
        }
    }

    fn setup(name: &str) -> (Economy, AuctionHouse, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("smp-ah-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut e = Economy::open(&dir, EcoConfig::default()).unwrap();
        e.now = T0;
        for (u, n) in [(ALEX, "Alex"), (STEVE, "Steve"), (NOTCH, "Notch")] {
            e.seen(u, n);
            e.adjust("sell", u, 1_000, "").unwrap();
        }
        let ah = AuctionHouse::open(&e, Config::default());
        (e, ah, dir)
    }

    /// Every coin of the game: balances and pools.
    fn total(e: &Economy) -> i64 {
        e.balance(ALEX) + e.balance(STEVE) + e.balance(NOTCH) + e.pool(POOL)
    }

    fn run(
        ah: &mut AuctionHouse,
        e: &mut Economy,
        me: &Player,
        bag: &mut Inv,
        now: i64,
        line: &str,
    ) -> Vec<Out> {
        match ah.command(e, me, true, bag, now, line) {
            Some(Reply::Out(o)) => o,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn a_fixed_sale_moves_items_once_and_pays_the_seller_the_whole_price() {
        let (mut e, mut ah, _dir) = setup("fixed");
        let mut alex = holding("diamond", 10);
        let out = run(
            &mut ah,
            &mut e,
            &p(ALEX, "Alex"),
            &mut alex,
            T0,
            "ah sell 1000",
        );
        assert!(
            matches!(&out[0], Out::Reply(s) if s.starts_with("Listed #1: 10 diamond for 1,000 coins (100.00 each)")),
            "{out:?}"
        );
        assert_eq!(
            alex.items["minecraft:diamond"], 0,
            "the items left the inventory"
        );
        assert_eq!(e.balance(ALEX), 1000, "listing is free");
        // Steve and Notch click "buy" in the same tick: one gets it.
        let mut steve = empty();
        let mut notch = empty();
        ah.online = [ALEX, STEVE, NOTCH].into();
        let a = ah.buy(&mut e, &p(STEVE, "Steve"), &mut steve, 1, T0 + 1);
        let b = ah.buy(&mut e, &p(NOTCH, "Notch"), &mut notch, 1, T0 + 1);
        assert!(
            matches!(&a[0], Out::Reply(s) if s.starts_with("Bought 10 diamond from Alex")),
            "{a:?}"
        );
        assert_eq!(
            b,
            [Out::Error(
                "That listing is gone (sold, withdrawn or ended).".into()
            )]
        );
        assert_eq!(
            (
                steve.items["minecraft:diamond"],
                notch.items.get("minecraft:diamond")
            ),
            (10, None)
        );
        // Alex gets the whole 1,000 Steve paid.
        assert_eq!(
            (e.balance(ALEX), e.balance(STEVE), e.balance(NOTCH)),
            (1000 + 1000, 0, 1000)
        );
        assert_eq!(total(&e), 3000, "nothing made, nothing lost");
        assert!(a.contains(&Out::Tell(
            ALEX,
            "Steve bought your 10 diamond for 1,000 coins.".into()
        )));
    }

    #[test]
    fn refusals_move_nothing() {
        let (mut e, mut ah, _dir) = setup("refuse");
        let alex = p(ALEX, "Alex");
        let mut bag = holding("diamond", 5);
        for (line, want) in [
            ("ah sell 0", "The price must be a positive whole number."),
            ("ah sell 10 6", "You hold 5 diamond."),
        ] {
            assert_eq!(
                run(&mut ah, &mut e, &alex, &mut bag, T0, line),
                [Out::Error(want.into())]
            );
        }
        let mut nothing = empty();
        assert_eq!(
            run(&mut ah, &mut e, &alex, &mut nothing, T0, "ah sell 10"),
            [Out::Error("Hold the items you want to sell.".into())]
        );
        // Creative players can't list or buy.
        assert!(matches!(
            ah.command(&mut e, &alex, false, &mut bag, T0, "ah sell 10"),
            Some(Reply::Out(o)) if matches!(&o[0], Out::Error(_))
        ));
        run(&mut ah, &mut e, &alex, &mut bag, T0, "ah sell 5000 5");
        // Own listing, too poor, no room, a failed give: nothing moves.
        assert!(
            matches!(&ah.buy(&mut e, &alex, &mut empty(), 1, T0)[0], Out::Error(s) if s.starts_with("That's your own"))
        );
        assert!(
            matches!(&ah.buy(&mut e, &p(STEVE, "Steve"), &mut empty(), 1, T0)[0], Out::Error(s) if s.starts_with("You only have"))
        );
        e.adjust("sell", STEVE, 10_000, "").unwrap();
        let mut full = Inv {
            slots_free: 0,
            ..Inv::default()
        };
        assert!(
            matches!(&ah.buy(&mut e, &p(STEVE, "Steve"), &mut full, 1, T0)[0], Out::Error(s) if s.starts_with("Make room"))
        );
        let mut broken = Inv {
            slots_free: 5,
            fail_give: true,
            ..Inv::default()
        };
        assert!(matches!(
            &ah.buy(&mut e, &p(STEVE, "Steve"), &mut broken, 1, T0)[0],
            Out::Error(_)
        ));
        assert!(ah.listing(1).is_some(), "the listing stays for sale");
        assert_eq!(e.balance(STEVE), 11_000);
        // Listings can exceed the old per-player cap even with a legacy config.
        ah.config.max_listings = 8;
        let mut many = holding("dirt", 64);
        for _ in 1..8 {
            run(&mut ah, &mut e, &alex, &mut many, T0, "ah sell 1 1");
        }
        let out = run(&mut ah, &mut e, &alex, &mut many, T0, "ah sell 1 1");
        assert!(matches!(&out[0], Out::Reply(_)), "{out:?}");
        assert_eq!(ah.data.listings.len(), 9);
    }

    #[test]
    fn auctions_hold_bids_refund_the_outbid_extend_late_bids_and_pay_an_offline_seller() {
        let (mut e, mut ah, dir) = setup("auction");
        let mut alex = holding("emerald", 3);
        run(
            &mut ah,
            &mut e,
            &p(ALEX, "Alex"),
            &mut alex,
            T0,
            "ah auction 100 2",
        );
        ah.online = [STEVE, NOTCH].into(); // Alex went offline.
        let steve = p(STEVE, "Steve");
        let notch = p(NOTCH, "Notch");
        assert!(
            matches!(&ah.bid(&mut e, &steve, 1, Some(99), T0)[0], Out::Error(s) if s.contains("100 coins"))
        );
        ah.bid(&mut e, &steve, 1, None, T0);
        assert_eq!((e.balance(STEVE), e.pool(POOL)), (900, 100));
        // 5% more at least: 105.
        assert!(matches!(
            &ah.bid(&mut e, &notch, 1, Some(104), T0)[0],
            Out::Error(_)
        ));
        let out = ah.bid(&mut e, &notch, 1, Some(200), T0);
        assert!(
            out.contains(&Out::Tell(
                STEVE,
                "You were outbid on #1 (3 emerald): 100 coins back. The bid is 200 coins.".into()
            )),
            "{out:?}"
        );
        assert_eq!(
            (e.balance(STEVE), e.balance(NOTCH), e.pool(POOL)),
            (1000, 800, 200)
        );
        // Notch raises his own bid: only the difference is held.
        ah.bid(&mut e, &notch, 1, Some(300), T0);
        assert_eq!((e.balance(NOTCH), e.pool(POOL)), (700, 300));
        // A bid 10 s before the end moves the end to 60 s from then.
        let late = T0 + 2 * 3_600_000 - 10_000;
        let out = ah.bid(&mut e, &steve, 1, Some(400), late);
        assert!(
            matches!(&out[0], Out::Reply(s) if s.contains("the end moved back")),
            "{out:?}"
        );
        assert_eq!(ah.listing(1).unwrap().ends, late + 60_000);
        assert!(
            ah.tick(&mut e, &[STEVE, NOTCH], late + 59_000).is_empty(),
            "not over yet"
        );
        // A restart mid-auction: everything comes back from economy.json.
        ah.store(&mut e);
        e.save().unwrap();
        let mut e = Economy::open(&dir, EcoConfig::default()).unwrap();
        let mut ah = AuctionHouse::open(&e, Config::default());
        assert_eq!((e.pool(POOL), ah.listing(1).unwrap().amount()), (400, 400));
        let out = ah.tick(&mut e, &[STEVE, NOTCH], late + 60_000);
        assert!(out.contains(&Out::Tell(
            STEVE,
            "You won #1 (3 emerald) for 400 coins: the items are in your mail (/ah claim).".into()
        )));
        assert_eq!(e.pool(POOL), 0);
        assert_eq!(
            e.balance(ALEX),
            1000 + 400,
            "paid while offline, the whole bid"
        );
        assert_eq!(e.balance(STEVE) + e.balance(NOTCH) + e.balance(ALEX), 3000);
        // Alex reads the note when back.
        let out = ah.tick(&mut e, &[ALEX], late + 61_000);
        assert_eq!(
            out,
            [Out::Tell(
                ALEX,
                "Auction #1 (3 emerald) sold to Steve for 400 coins.".into()
            )]
        );
        // Steve claims: the items, once.
        let mut bag = empty();
        let out = ah.claim(STEVE, &mut bag, None);
        assert_eq!(out, [Out::Reply("From your mail: 3 emerald.".into())]);
        assert_eq!(
            ah.claim(STEVE, &mut bag, None),
            [Out::Reply("Your mail is empty.".into())]
        );
        assert_eq!(bag.items["minecraft:emerald"], 3);
    }

    #[test]
    fn unsold_listings_come_back_by_mail_and_a_full_inventory_keeps_the_rest() {
        let (mut e, mut ah, _dir) = setup("expire");
        let alex = p(ALEX, "Alex");
        let mut bag = holding("wheat", 64);
        run(&mut ah, &mut e, &alex, &mut bag, T0, "ah sell 100");
        assert_eq!(
            run(&mut ah, &mut e, &alex, &mut bag, T0, "ah sell 100"),
            [Out::Error("Hold the items you want to sell.".into())]
        );
        let mut more = holding("wheat", 64);
        run(&mut ah, &mut e, &alex, &mut more, T0, "ah auction 50 1");
        ah.online = [ALEX].into();
        // The auction (1 h) ends with no bid; the fixed one (48 h) later.
        let out = ah.tick(&mut e, &[ALEX], T0 + 3_600_000);
        assert_eq!(
            out,
            [Out::Tell(
                ALEX,
                "#2 (64 wheat) ended with no bids: the items are in your mail (/ah claim).".into()
            )]
        );
        ah.tick(&mut e, &[ALEX], T0 + 48 * 3_600_000);
        assert_eq!(ah.data.listings.len(), 0);
        assert_eq!(ah.mail_of(ALEX).len(), 2);
        // Room for one stack only: one parcel comes, the other waits.
        let mut bag = Inv {
            slots_free: 1,
            ..Inv::default()
        };
        let mut room_once = |ah: &mut AuctionHouse| ah.claim(ALEX, &mut bag, Some(0));
        assert_eq!(
            room_once(&mut ah),
            [Out::Reply("From your mail: 64 wheat.".into())]
        );
        assert_eq!(ah.mail_of(ALEX).len(), 1);
        let mut full = Inv {
            slots_free: 0,
            ..Inv::default()
        };
        assert_eq!(
            ah.claim(ALEX, &mut full, None),
            [Out::Error(
                "Your inventory is full: the rest waits in your mail.".into()
            )]
        );
        assert_eq!(ah.mail_of(ALEX).len(), 1, "nothing lost");
        // Withdrawing: back by mail; an auction with a bid can't be withdrawn.
        let mut dirt = holding("dirt", 10);
        run(&mut ah, &mut e, &alex, &mut dirt, T0, "ah auction 10 1");
        ah.bid(&mut e, &p(STEVE, "Steve"), 3, None, T0);
        assert_eq!(
            ah.withdraw(&alex, 3),
            [Out::Error("Someone bid on it: it runs to its end.".into())]
        );
        assert_eq!(
            ah.withdraw(&p(STEVE, "Steve"), 3),
            [Out::Error("That isn't your listing.".into())]
        );
    }

    #[test]
    fn the_browser_sorts_by_unit_price_and_filters() {
        let (mut e, mut ah, _dir) = setup("menu");
        let alex = p(ALEX, "Alex");
        for (item, n, price) in [
            ("diamond", 2, 300),
            ("wheat", 64, 64),
            ("diamond", 1, 120),
            ("oak_log", 32, 16),
        ] {
            let mut bag = holding(item, n);
            run(
                &mut ah,
                &mut e,
                &alex,
                &mut bag,
                T0,
                &format!("ah sell {price}"),
            );
        }
        let cats = super::super::smp_shop::Config::default().categories;
        let all: Vec<u64> = ah
            .filtered(&Filter::All, &cats, T0)
            .iter()
            .map(|l| l.id)
            .collect();
        assert_eq!(all, [4, 2, 3, 1], "0.5, 1, 120, 150 each");
        let mining = cats.iter().position(|c| c.name == "Mining").unwrap();
        let ids: Vec<u64> = ah
            .filtered(&Filter::Category(mining), &cats, T0)
            .iter()
            .map(|l| l.id)
            .collect();
        assert_eq!(ids, [3, 1]);
        let ids: Vec<u64> = ah
            .filtered(&Filter::Search("log".into()), &cats, T0)
            .iter()
            .map(|l| l.id)
            .collect();
        assert_eq!(ids, [4]);
        assert_eq!(
            category_of("minecraft:netherite_sword", &cats),
            cats.iter().position(|c| c.name == "Tools").unwrap()
        );
        assert_eq!(category_of("minecraft:music_disc_cat", &cats), cats.len());
        // Home → All → a listing → confirm → buy.
        ah.views.insert(STEVE, (Some(1), Page::Home));
        assert_eq!(ah.click(STEVE, &cats, 10, T0), Click::Redraw);
        assert!(matches!(ah.views[&STEVE].1, Page::List(Filter::All, 0)));
        assert_eq!(ah.layout(STEVE, &cats, T0)[0], Cell::Listing(4));
        assert_eq!(ah.click(STEVE, &cats, 0, T0), Click::Redraw);
        assert_eq!(ah.click(STEVE, &cats, CONFIRM, T0), Click::Buy(4));
        assert_eq!(ah.click(STEVE, &cats, CANCEL, T0), Click::Redraw);
        assert!(
            matches!(ah.views[&STEVE].1, Page::List(Filter::All, 0)),
            "cancel goes back to the list"
        );
        let money = |n: i64| format!("{n} coins");
        let l = ah.listing(4).unwrap();
        assert!(
            ah.label(l, T0, &money)
                .starts_with("#4 32 Oak Log · 16 coins (0.50 each) · Alex · 2d 0h left")
        );
    }
}
