// `/shop`: the market (smp_market) as a chest menu. The first page shows the
// categories; a category's page its items, with what the market asks and
// pays (in its name). A click buys one, a shift-click a stack of 64; a click on a stack in
// your own inventory sells it; "Sell all" sells everything the market buys.
// Buying and selling go through the market's own rules (its prices,
// items first then money, logged). This file only lays the pages out
// and says what a click means; the glue opens the menu and moves the items.
// No game types. Added to the game as `game::smp_shop`; also a module of this
// crate, for its tests.

use super::smp_market::{Config as MarketConfig, Price, item_name};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Rows of the menu, and its slots.
pub const ROWS: u8 = 6;
pub const SIZE: usize = 54;
/// A page's item slots (the rows above the bottom one).
pub const PER_PAGE: usize = 45;
/// What a shift-click buys.
pub const STACK: u32 = 64;

/// The bottom row's buttons.
const BACK: usize = 45;
const HELP: usize = 46;
const PREV: usize = 47;
const BALANCE: usize = 48;
const SELL_ALL: usize = 49;
const LIMITS: usize = 50;
const NEXT: usize = 51;
const CLOSE: usize = 53;
/// Where the first page puts the categories: the middle of rows 2 to 4.
const CATEGORY_SLOTS: [usize; 21] = [
    10, 11, 12, 13, 14, 15, 16, 19, 20, 21, 22, 23, 24, 25, 28, 29, 30, 31, 32, 33, 34,
];

/// A category: its name, the item its button shows, its items.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Category {
    pub name: String,
    pub icon: String,
    pub items: Vec<String>,
}

/// `config.json`'s `shop`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub title: String,
    /// The market's items not in any category go to an "Other" one.
    pub categories: Vec<Category>,
}

impl Default for Config {
    fn default() -> Self {
        let cat = |name: &str, icon: &str, items: &[&str]| Category {
            name: name.into(),
            icon: format!("minecraft:{icon}"),
            items: items.iter().map(|i| format!("minecraft:{i}")).collect(),
        };
        Self {
            enabled: true,
            title: "Shop".into(),
            categories: vec![
                cat(
                    "Farming",
                    "wheat",
                    &[
                        "wheat",
                        "wheat_seeds",
                        "carrot",
                        "potato",
                        "beetroot",
                        "sugar_cane",
                        "pumpkin",
                        "melon_slice",
                        "cactus",
                        "bone_meal",
                    ],
                ),
                cat(
                    "Mining",
                    "diamond",
                    &[
                        "coal",
                        "copper_ingot",
                        "iron_ingot",
                        "gold_ingot",
                        "redstone",
                        "lapis_lazuli",
                        "quartz",
                        "emerald",
                        "diamond",
                    ],
                ),
                cat(
                    "Wood",
                    "oak_log",
                    &[
                        "oak_log",
                        "spruce_log",
                        "birch_log",
                        "jungle_log",
                        "acacia_log",
                        "dark_oak_log",
                        "mangrove_log",
                        "cherry_log",
                    ],
                ),
                cat(
                    "Mob drops",
                    "bone",
                    &[
                        "rotten_flesh",
                        "bone",
                        "string",
                        "gunpowder",
                        "spider_eye",
                        "slime_ball",
                        "blaze_rod",
                        "leather",
                        "feather",
                    ],
                ),
                cat(
                    "Building",
                    "bricks",
                    &[
                        "cobblestone",
                        "stone",
                        "dirt",
                        "sand",
                        "gravel",
                        "glass",
                        "bricks",
                        "stone_bricks",
                    ],
                ),
                cat(
                    "Food",
                    "bread",
                    &[
                        "bread",
                        "cooked_beef",
                        "cooked_porkchop",
                        "cooked_chicken",
                        "baked_potato",
                        "apple",
                        "golden_carrot",
                    ],
                ),
                cat(
                    "Tools",
                    "torch",
                    &["torch", "ladder", "crafting_table", "chest", "furnace"],
                ),
            ],
        }
    }
}

/// Which page a player sees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Page {
    Main,
    /// A category (by index in `categories`) and its page from 0.
    Category(usize, usize),
}

/// One slot of the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    /// The bottom row's background.
    Filler,
    /// A category's button: its icon, name, and how many items it has.
    Category(String, String, usize),
    /// An item and its price.
    Item(String, Price),
    Back,
    Prev,
    Next,
    Balance,
    SellAll,
    Limits,
    Help,
    Close,
}

/// What a click asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    /// Nothing (an empty slot, a filler).
    Nothing,
    /// Show this page.
    Show(Page),
    /// Buy this many of the item.
    Buy(String, u32),
    /// Sell everything the market buys.
    SellAll,
    Close,
}

/// The categories as shown: each with only the items the market trades,
/// those it trades in none of them under "Other", the empty ones left out.
pub fn categories(config: &Config, market: &MarketConfig) -> Vec<Category> {
    let traded = |id: &String| {
        market
            .items
            .get(id)
            .is_some_and(|p| p.buy > 0 || p.sell > 0)
    };
    let mut out: Vec<Category> = config
        .categories
        .iter()
        .map(|c| Category {
            items: c.items.iter().filter(|i| traded(i)).cloned().collect(),
            ..c.clone()
        })
        .collect();
    let other: Vec<String> = market
        .items
        .keys()
        .filter(|id| traded(id) && !out.iter().any(|c| c.items.contains(id)))
        .cloned()
        .collect();
    if !other.is_empty() {
        out.push(Category {
            name: "Other".into(),
            icon: "minecraft:chest".into(),
            items: other,
        });
    }
    out.retain(|c| !c.items.is_empty());
    out
}

/// The players' open shop menus: their window and page.
#[derive(Default)]
pub struct Shop {
    pub views: HashMap<u128, (Option<i32>, Page)>,
}

impl Shop {
    /// The page `uuid` sees, as cells.
    pub fn layout(&self, config: &Config, market: &MarketConfig, uuid: u128) -> Vec<Cell> {
        let page = self.views.get(&uuid).map_or(Page::Main, |v| v.1);
        let cats = categories(config, market);
        let mut cells = vec![Cell::Empty; SIZE];
        for c in &mut cells[PER_PAGE..] {
            *c = Cell::Filler;
        }
        cells[HELP] = Cell::Help;
        cells[BALANCE] = Cell::Balance;
        cells[SELL_ALL] = Cell::SellAll;
        cells[LIMITS] = Cell::Limits;
        cells[CLOSE] = Cell::Close;
        match page {
            Page::Main => {
                for (slot, c) in CATEGORY_SLOTS.iter().zip(&cats) {
                    cells[*slot] = Cell::Category(c.icon.clone(), c.name.clone(), c.items.len());
                }
            }
            Page::Category(i, p) => {
                cells[BACK] = Cell::Back;
                let items = cats.get(i).map(|c| c.items.as_slice()).unwrap_or_default();
                for (slot, id) in items.iter().skip(p * PER_PAGE).take(PER_PAGE).enumerate() {
                    cells[slot] = Cell::Item(
                        id.clone(),
                        market.items.get(id).copied().unwrap_or_default(),
                    );
                }
                if p > 0 {
                    cells[PREV] = Cell::Prev;
                }
                if (p + 1) * PER_PAGE < items.len() {
                    cells[NEXT] = Cell::Next;
                }
            }
        }
        cells
    }

    /// What a click on the menu's `slot` means (`shift`: a shift-click); a
    /// new page is kept as the player's.
    pub fn click(
        &mut self,
        config: &Config,
        market: &MarketConfig,
        uuid: u128,
        slot: usize,
        shift: bool,
    ) -> Click {
        let cells = self.layout(config, market, uuid);
        let page = self.views.get(&uuid).map_or(Page::Main, |v| v.1);
        let click = match (cells.get(slot), page) {
            (Some(Cell::Category(_, name, _)), _) => {
                let i = categories(config, market)
                    .iter()
                    .position(|c| &c.name == name);
                i.map_or(Click::Nothing, |i| Click::Show(Page::Category(i, 0)))
            }
            (Some(Cell::Item(id, price)), _) if price.buy > 0 => {
                Click::Buy(id.clone(), if shift { STACK } else { 1 })
            }
            (Some(Cell::Back), _) => Click::Show(Page::Main),
            (Some(Cell::Prev), Page::Category(i, p)) => {
                Click::Show(Page::Category(i, p.saturating_sub(1)))
            }
            (Some(Cell::Next), Page::Category(i, p)) => Click::Show(Page::Category(i, p + 1)),
            (Some(Cell::SellAll), _) => Click::SellAll,
            (Some(Cell::Close), _) => Click::Close,
            _ => Click::Nothing,
        };
        if let (Click::Show(p), Some(v)) = (&click, self.views.get_mut(&uuid)) {
            v.1 = *p;
        }
        click
    }
}

/// An item's button: its name and what the market asks and pays.
pub fn item_label(id: &str, price: Price, money: &dyn Fn(i64) -> String) -> String {
    let mut parts = Vec::new();
    if price.buy > 0 {
        parts.push(format!("buy {}", money(price.buy)));
    }
    if price.sell > 0 {
        parts.push(format!("sell {}", money(price.sell)));
    }
    format!("{}: {}", title(id), parts.join(", "))
}

/// The help button's text.
pub const HELP_TEXT: &str = "Click: buy 1 · Shift-click: buy 64 · Click a stack of yours: sell it";

/// `minecraft:iron_ingot` as `Iron Ingot`, for a button.
pub fn title(id: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    const ALEX: u128 = 1;

    fn open(shop: &mut Shop) {
        shop.views.insert(ALEX, (Some(3), Page::Main));
    }

    #[test]
    fn the_first_page_shows_the_categories_with_the_market_s_items() {
        let (config, mut market) = (Config::default(), MarketConfig::default());
        // An item in no category: under "Other". One the market doesn't
        // trade: nowhere.
        market
            .items
            .insert("minecraft:sponge".into(), Price { buy: 50, sell: 0 });
        market.items.remove("minecraft:torch");
        let cats = categories(&config, &market);
        assert_eq!(cats.last().unwrap().name, "Other");
        assert_eq!(cats.last().unwrap().items, ["minecraft:sponge"]);
        assert!(
            cats.iter()
                .all(|c| !c.items.contains(&"minecraft:torch".to_owned()))
        );
        // Every default item is in a category of its own.
        let all = categories(&config, &MarketConfig::default());
        assert!(all.iter().all(|c| c.name != "Other"), "{all:?}");
        let mut shop = Shop::default();
        open(&mut shop);
        let cells = shop.layout(&config, &market, ALEX);
        assert_eq!(
            cells[10],
            Cell::Category("minecraft:wheat".into(), "Farming".into(), 10)
        );
        assert_eq!(
            (cells[BALANCE].clone(), cells[CLOSE].clone()),
            (Cell::Balance, Cell::Close)
        );
        assert_eq!(cells[BACK], Cell::Filler);
    }

    #[test]
    fn clicks_open_a_category_buy_one_or_a_stack_and_go_back() {
        let (config, market) = (Config::default(), MarketConfig::default());
        let mut shop = Shop::default();
        open(&mut shop);
        // Mining: the second category.
        assert_eq!(
            shop.click(&config, &market, ALEX, 11, false),
            Click::Show(Page::Category(1, 0))
        );
        let cells = shop.layout(&config, &market, ALEX);
        assert_eq!(
            cells[0],
            Cell::Item("minecraft:coal".into(), Price { buy: 4, sell: 4 })
        );
        assert_eq!(cells[BACK], Cell::Back);
        assert_eq!(
            shop.click(&config, &market, ALEX, 0, false),
            Click::Buy("minecraft:coal".into(), 1)
        );
        assert_eq!(
            shop.click(&config, &market, ALEX, 0, true),
            Click::Buy("minecraft:coal".into(), 64)
        );
        assert_eq!(
            shop.click(&config, &market, ALEX, 40, false),
            Click::Nothing
        );
        assert_eq!(
            shop.click(&config, &market, ALEX, SELL_ALL, false),
            Click::SellAll
        );
        assert_eq!(
            shop.click(&config, &market, ALEX, BACK, false),
            Click::Show(Page::Main)
        );
        assert_eq!(shop.views[&ALEX].1, Page::Main);
        assert_eq!(
            shop.click(&config, &market, ALEX, CLOSE, false),
            Click::Close
        );
    }

    #[test]
    fn an_item_the_market_only_buys_is_not_sold_and_long_categories_have_pages() {
        let mut market = MarketConfig::default();
        let mut config = Config::default();
        // Rotten flesh: the market buys it, never sells it.
        let mut shop = Shop::default();
        open(&mut shop);
        shop.click(&config, &market, ALEX, 13, false);
        assert_eq!(shop.views[&ALEX].1, Page::Category(3, 0));
        assert_eq!(shop.click(&config, &market, ALEX, 0, false), Click::Nothing);
        // 50 items in one category: two pages.
        let many: Vec<String> = (0..50).map(|i| format!("minecraft:item_{i}")).collect();
        for id in &many {
            market.items.insert(id.clone(), Price { buy: 2, sell: 1 });
        }
        config.categories = vec![Category {
            name: "All".into(),
            icon: "minecraft:chest".into(),
            items: many,
        }];
        shop.views.insert(ALEX, (Some(3), Page::Category(0, 0)));
        let cells = shop.layout(&config, &market, ALEX);
        assert_eq!(
            (cells[PREV].clone(), cells[NEXT].clone()),
            (Cell::Filler, Cell::Next)
        );
        assert_eq!(
            shop.click(&config, &market, ALEX, NEXT, false),
            Click::Show(Page::Category(0, 1))
        );
        let cells = shop.layout(&config, &market, ALEX);
        assert_eq!(
            cells[0],
            Cell::Item("minecraft:item_45".into(), Price { buy: 2, sell: 1 })
        );
        assert_eq!(cells[5], Cell::Empty);
        assert_eq!(cells[PREV], Cell::Prev);
    }

    #[test]
    fn labels_read_well() {
        let money = |n: i64| format!("{n} coins");
        assert_eq!(
            item_label("minecraft:iron_ingot", Price { buy: 30, sell: 8 }, &money),
            "Iron Ingot: buy 30 coins, sell 8 coins"
        );
        assert_eq!(
            item_label("minecraft:bone", Price { buy: 0, sell: 1 }, &money),
            "Bone: sell 1 coins"
        );
    }
}
