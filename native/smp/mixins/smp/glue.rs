// The SMP in the server's game: its state for the open world, and the edges
// between the game's types and the rules (smp_economy, smp_market, smp_shop,
// smp_trade, smp_games, smp_lottery, smp_places, smp_claims). Added to
// the game as `game::smp_glue`; the helpers (register.rs, command.rs,
// tick.rs, level_tick.rs, lifecycle.rs, menu_clicked.rs, menu_closed.rs, destroy_block.rs,
// right_click_block.rs, block_spread.rs, piston_move.rs,
// explosion_detonate.rs, fill_bucket.rs, attack_entity.rs) call it from the
// hook points.

use crate::game::{
    Game,
    smp_admin::{self, LastPositions},
    smp_auction::{self, AuctionHouse},
    smp_report::{self, Report},
    smp_ledger,
    smp_reports::{self, Reports},
    native_server::{NativeCommand, NativeServerPlayer, NativeSidebar},
    smp_board::{self, Board, Line},
    smp_casino::{self, Casino},
    smp_casino_glue,
    smp_claims::{self, Claims},
    smp_economy::{self, Economy, Out, Player},
    smp_games::{self, Games},
    smp_lottery::Lottery,
    smp_places::{self, Body, Place, Places},
    smp_presence::{self, Presence},
    native_menus::native_menu_inventory_slot,
    smp_market::{self, Bag, Market},
    smp_market_chart, smp_market_menu, smp_market_prices,
    smp_nick::{self, Names as _, Nicks},
    smp_shop::{self, Shop},
    smp_store,
    smp_switch::{self, Part, Switches},
    smp_trade::{self, Act, Cell, Trades},
    smp_vanish::{self, Vanish},
};
use horizon_client::{
    items::ItemStack,
    lan::data::component_nbt,
    text::{self, Text},
};
use std::{path::PathBuf, sync::Mutex};

/// The mod's id, name and version.
pub const ID: &str = "smp";
pub const NAME: &str = "SMP";
pub const VERSION: &str = "0.36.1";

/// `config.json` in `world/horizon-smp/`, written with its defaults the
/// first time, and again with the keys a newer version adds (values kept).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(flatten)]
    pub economy: smp_economy::Config,
    pub market: smp_market::Config,
    pub shop: smp_shop::Config,
    pub gambling: smp_games::Config,
    pub casino: smp_casino::Config,
    pub places: smp_places::Config,
    pub claims: smp_claims::Config,
    pub board: smp_board::Config,
    pub presence: smp_presence::Config,
    pub nicks: smp_nick::Config,
    pub auction: smp_auction::Config,
    pub report: smp_report::Config,
    /// `/report`: players telling the staff about another player.
    pub reports: smp_reports::Config,
    pub web: WebPush,
}

/// `config.json`'s `web`: where the web pages (`web/market.json`,
/// `web/economy.json`) are sent for the website, `url` + `/<name>`, with
/// `token` as the bearer (off while empty). Only what changed is sent, at
/// most once a minute, gzipped, by the thread that writes the market page.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WebPush {
    pub url: String,
    pub token: String,
}

impl Default for WebPush {
    fn default() -> Self {
        Self {
            url: "https://kira-production-500e.up.railway.app/smp/ingest".into(),
            token: String::new(),
        }
    }
}

/// The SMP of the open world.
pub struct Smp {
    dir: PathBuf,
    /// An error when `economy.json` can't be read: the commands say so.
    economy: Result<Economy, String>,
    /// The auction house (None with the economy off).
    auction: Option<AuctionHouse>,
    /// `/eco stats`, the daily report, `web/economy.json`.
    report: Option<Report>,
    market: Market,
    shop_config: smp_shop::Config,
    /// The `/shop` menus open.
    shop: Shop,
    /// The `/market` menus open.
    market_menus: smp_market_menu::Menus,
    /// When `web/market.json` was last written (ms).
    market_web: i64,
    /// Where the web pages are sent.
    web: WebPush,
    trades: Trades<ItemStack>,
    games: Games,
    lottery: Lottery,
    /// The casino's menus and rounds.
    casino: Casino,
    /// An error when `places.json` can't be read: the commands say so.
    places: Result<Places, String>,
    /// An error when `claims.json` can't be read: nothing is guarded then,
    /// and the commands say so.
    claims: Result<Claims, String>,
    /// An error when `board.json` can't be read: no sidebar then.
    board: Result<Board, String>,
    /// An error when `presence.json` can't be read: no welcome, reward or
    /// AFK then.
    presence: Result<Presence, String>,
    /// An error when `nicks.json` can't be read: nicknames are off then.
    nicks: Result<Nicks, String>,
    /// The players on whose nickname the game shows (this run).
    nicks_shown: std::collections::HashSet<u128>,
    /// `/claim show`: whose chunk border is drawn (level, chunk x, chunk z)
    /// until when.
    shows: std::collections::HashMap<u128, (String, i32, i32, i64)>,
    /// `/vanish`: an error when `vanish.json` can't be read (nobody is
    /// hidden then, and the command says so).
    vanish: Result<Vanish, String>,
    /// The vanished players on who were sent the subtitle: cleared and
    /// counted for sleeping again once they show themselves or leave.
    vanish_titled: std::collections::HashSet<u128>,
    /// The parts operators turned off (`/smp off`).
    switches: Switches,
    /// `/report` and `/reports`.
    reports: Reports,
    /// Where each player last left (`/smpadmin lastpos`): an error when
    /// `last_positions.json` can't be read (nothing is kept then).
    last: Result<LastPositions, String>,
    /// Where each player on stood at the last second's look: written to
    /// `last` when they leave (they are off the player list by then).
    standing: std::collections::HashMap<u128, Place>,
}

static STATE: Mutex<Option<Smp>> = Mutex::new(None);
static TICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Whether the next tick has work (a teleport warming up, a trade open, a
/// command's changes to write): otherwise only one tick a second does any.
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// Sleep-count changes decided inside a player's command, for the next
/// second's tick.
static PENDING_SLEEP: Mutex<Vec<smp_presence::Act>> = Mutex::new(Vec::new());
/// Whether an `/rtp` waits for its land: the Overworld's tick reads it then.
static RTP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The owner of the chunk tickets `/rtp` holds while it looks for land.
const RTP_TICKET: &str = "smp:rtp";
/// The players who left (`player_left`), with where they stood when the
/// server still listed them, and when.
static LEFT: Mutex<Vec<(u128, String, Option<Place>, i64)>> = Mutex::new(Vec::new());
/// What each `/rtp`'s reads cost the Overworld's tick, by player: reads,
/// total and longest (µs), for the line logged when they land.
static RTP_COST: Mutex<Vec<(u128, u32, u64, u64)>> = Mutex::new(Vec::new());

fn open(dir: PathBuf) -> Smp {
    let config_path = dir.join("config.json");
    let (config, write) = match smp_store::read_json::<Option<serde_json::Value>>(&config_path) {
        // No file yet: the defaults, written for the owner to edit.
        Ok(None) => (Config::default(), true),
        Ok(Some(v)) => match serde_json::from_value::<Config>(v.clone()) {
            // Written again only when a newer version adds keys; values kept.
            Ok(c) => {
                let keys = |v: &serde_json::Value| v.as_object().map(|o| o.len());
                let full = serde_json::to_value(&c).unwrap_or_default();
                let added = keys(&full) != keys(&v);
                (c, added)
            }
            Err(e) => {
                log::error!(
                    "[smp] {}: {e}: the defaults are used",
                    config_path.display()
                );
                (Config::default(), false)
            }
        },
        // A damaged file stays as it is for the owner to mend.
        Err(e) => {
            log::error!("[smp] {e}: the defaults are used");
            (Config::default(), false)
        }
    };
    let mut config = config;
    // Prices never edited: today's defaults (the balances are untouched).
    let write = smp_market::migrate(&mut config.market) | write;
    // 0.13's sidebar title, never edited: today's.
    let write = smp_board::migrate(&mut config.board) | write;
    if write && let Err(e) = smp_store::write_json(&config_path, &config) {
        log::error!("[smp] {e}");
    }
    log::info!("[smp] {NAME} {VERSION} on {}", dir.display());
    let (market, problems) = Market::open(&dir, config.market);
    for p in problems {
        log::warn!("[smp] {p}");
    }
    let economy = Economy::open(&dir, config.economy);
    if let Err(e) = &economy {
        log::error!("[smp] the economy is off: {e}");
    }
    let auction = economy.as_ref().ok().map(|e| AuctionHouse::open(e, config.auction));
    let report = economy.as_ref().ok().map(|e| Report::open(e, config.report));
    let reports = Reports::open(&dir, config.reports);
    let places = Places::open(&dir, config.places);
    if let Err(e) = &places {
        log::error!("[smp] homes and warps are off: {e}");
    }
    let mut claims = Claims::open(&dir, config.claims);
    match &mut claims {
        Ok(c) => {
            for line in c.clear_free_zone() {
                log::warn!("[smp] {line}");
            }
        }
        Err(e) => log::error!("[smp] claims are off: {e}"),
    }
    let board = Board::open(&dir, config.board);
    let vanish = Vanish::open(&dir);
    if let Err(e) = &vanish {
        log::error!("[smp] /vanish is off: {e}");
    }
    if let Err(e) = &board {
        log::error!("[smp] the sidebar is off: {e}");
    }
    // Players the economy knows were here before: no first-join welcome.
    let known = economy.as_ref().map(|e| e.accounts().into_iter().map(|a| a.0).collect()).unwrap_or_else(|_| Vec::new());
    let presence = Presence::open(&dir, config.presence, known);
    let mut nicks = Nicks::open(&dir, config.nicks);
    if let Err(e) = &nicks {
        log::error!("[smp] nicknames are off: {e}");
    }
    // Nicknames an older version let through that today's rules refuse.
    if let Ok(n) = &mut nicks {
        let names = ServerNames::read(dir.parent().unwrap_or(&dir), economy.as_ref().ok());
        for (uuid, nick, why) in n.purge(&names) {
            let real = names.name_of(uuid).unwrap_or_else(|| smp_store::uuid_string(uuid));
            log::warn!("[smp] nickname {nick} of {real} taken off: {why}");
        }
        if let Err(e) = n.save() {
            log::error!("[smp] {e}");
        }
    }
    if let Err(e) = &presence {
        log::error!("[smp] welcomes, daily rewards and AFK are off: {e}");
    }
    let (switches, problem) = Switches::open(&dir);
    if let Some(e) = problem {
        log::error!("[smp] {e}: every part is on");
    }
    for p in Part::ALL.into_iter().filter(|&p| !switches.on(p)) {
        log::warn!("[smp] {} is turned off (/smp on {} turns it on)", p.name(), p.name());
    }
    let last = LastPositions::open(&dir);
    if let Err(e) = &last {
        log::error!("[smp] last positions are off: {e}");
    }
    let (casino, problem) = Casino::open(&dir, config.casino, config.gambling.enabled);
    if let Some(e) = problem {
        log::error!("[smp] casino.json can't be read, the day's ledger starts again (kept apart): {e}");
    }
    Smp {
        dir,
        economy,
        auction,
        report,
        market,
        shop_config: config.shop,
        shop: Shop::default(),
        market_menus: Default::default(),
        market_web: 0,
        web: config.web.clone(),
        trades: Trades::default(),
        games: Games::new(config.gambling),
        lottery: Lottery::default(),
        casino,
        places,
        claims,
        board,
        presence,
        nicks,
        nicks_shown: Default::default(),
        shows: Default::default(),
        vanish,
        vanish_titled: Default::default(),
        switches,
        reports,
        last,
        standing: Default::default(),
    }
}

/// The SMP's state, opened for the world in `dir` on first use (the files
/// of another world it held are written first).
fn state(dir: PathBuf) -> std::sync::MutexGuard<'static, Option<Smp>> {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.as_ref().is_none_or(|s| s.dir != dir) {
        if let Some(old) = state.as_mut() {
            save_now(old);
        }
        *state = Some(open(dir));
    }
    if let Some(Ok(e)) = state.as_mut().map(|s| &mut s.economy) {
        e.now = chrono::Utc::now().timestamp_millis();
    }
    state
}

fn save(smp: &mut Smp) {
    use horizon_client::spans::guard;
    let _span = guard("smp_save");
    // The report's state is written with the balances, in the same file.
    {
        let _s = guard("smp_save_report_store");
        if let (Some(r), Ok(e)) = (&mut smp.report, &mut smp.economy) {
            r.store(e);
        }
    }
    // So are the auction house's listings, bids and mail.
    {
        let _s = guard("smp_save_auction_store");
        if let (Some(a), Ok(e)) = (&mut smp.auction, &mut smp.economy) {
            a.store(e);
            for p in a.problems.drain(..) {
                log::error!("[smp] {p}");
            }
        }
    }
    {
        let _s = guard("smp_save_economy");
        if let Ok(e) = &mut smp.economy
            && let Err(err) = e.save()
        {
            log::error!("[smp] {err}");
        }
    }
    {
        let _s = guard("smp_save_places");
        if let Ok(p) = &mut smp.places
            && let Err(err) = p.save(chrono::Utc::now().timestamp_millis())
        {
            log::error!("[smp] {err}");
        }
    }
    {
        let _s = guard("smp_save_claims");
        if let Ok(c) = &mut smp.claims
            && let Err(err) = c.save()
        {
            log::error!("[smp] {err}");
        }
    }
    if let Ok(b) = &mut smp.board
        && let Err(err) = b.save()
    {
        log::error!("[smp] {err}");
    }
    {
        let _s = guard("smp_save_presence");
        if let Ok(p) = &mut smp.presence
            && let Err(err) = p.save()
        {
            log::error!("[smp] {err}");
        }
    }
    if let Ok(n) = &mut smp.nicks
        && let Err(err) = n.save()
    {
        log::error!("[smp] {err}");
    }
    if let Err(err) = smp.casino.save() {
        log::error!("[smp] {err}");
    }
    if let Ok(v) = &mut smp.vanish
        && let Err(err) = v.save()
    {
        log::error!("[smp] {err}");
    }
    if let Err(err) = smp.switches.save() {
        log::error!("[smp] {err}");
    }
    smp.reports.save();
    if let Ok(l) = &mut smp.last
        && let Err(err) = l.save()
    {
        log::error!("[smp] {err}");
    }
    // The files above are written by the SMP's writer thread.
    for err in smp_store::take_errors() {
        log::error!("[smp] {err}");
    }
}

/// Every file of `smp` on disk now (the world closes, another opens).
fn save_now(smp: &mut Smp) {
    note_leavers(smp);
    flush_book(smp);
    save(smp);
    for err in smp_store::flush() {
        log::error!("[smp] {err}");
    }
}

/// The commands, for every player's command tree.
pub fn commands(commands: &mut Vec<NativeCommand>) {
    for usage in [
        "balance [player:word]",
        "bal [player:word]",
        "pay <player:word> <amount:integer>",
        "baltop [page:integer]",
        "worth [item:item]",
        "sell hand [count:integer]",
        "sell all",
        "sell <item:item> [count:integer]",
        "buy <item:item> [count:integer]",
        "shop",
        "market",
        "market <item:item>",
        "trade <player:word>",
        "trade accept [player:word]",
        "trade deny [player:word]",
        "trade cancel",
        "trade money <amount:integer>",
        "coinflip <amount:integer> [player:word]",
        "coinflip accept <player:word>",
        "coinflip cancel",
        "coinflip list",
        "lottery",
        "lottery buy <tickets:integer>",
        "casino",
        "casino help",
        "casino top",
        "casino stats",
        "casino roulette",
        "casino slots",
        "casino crash",
        "casino blackjack",
        "spawn",
        "home [name:word]",
        "sethome [name:word]",
        "delhome [name:word]",
        "homes",
        "warp <name:word>",
        "warps",
        "tpa <player:word>",
        "tpaccept [player:word]",
        "tpdeny [player:word]",
        "rules",
        "back",
        "rtp",
        "wild",
        "playtime [player:word]",
        "claim",
        "claim info",
        "claim show",
        "unclaim",
        "claims",
        "claims map",
        "trust <player:word>",
        "untrust <player:word>",
        "sidebar",
        "sidebar on",
        "sidebar off",
        "afk",
        "nick <name:word>",
        "realname <nick:word>",
        "ah",
        "ah search <words:text>",
        "ah sell <price:integer> [count:integer]",
        "ah auction <start:integer> [hours:integer]",
        "ah bid <listing:integer> <amount:integer>",
        "ah buy <listing:integer>",
        "ah cancel <listing:integer>",
        "ah claim",
        "ah mine",
        "ah mail",
        "eco stats",
        "money stats",
        "report <player:word> [reason:text]",
    ] {
        commands.push(NativeCommand::new(usage));
    }
    for usage in [
        "setspawn",
        "setwarp <name:word>",
        "delwarp <name:word>",
        "nick <name:word> <nick:word>",
        "vanish",
        "vanish list",
        "reports [page:integer]",
        "ledger",
        "ledger minted",
        "ledger <player:word> [page:integer]",
    ] {
        commands.push(NativeCommand::op(usage, 3));
    }
    for usage in smp_admin::USAGES {
        commands.push(NativeCommand::op(usage, smp_admin::LEVEL));
    }
    for usage in ["smp", "smp status", "smp on [part:word]", "smp off [part:word] [reason:text]"] {
        commands.push(NativeCommand::op(usage, smp_switch::LEVEL));
    }
    // Operators make money (an event prize, a bug refund: with a reason
    // everyone reads) and take a cheat's gains away.
    for name in ["eco", "money"] {
        commands.push(NativeCommand::op(&format!("{name} give <player:word> <amount:integer> <reason:text>"), 3));
        commands.push(NativeCommand::op(&format!("{name} take <player:word> <amount:integer> [reason:text]"), 3));
    }
}

/// The acting player's items (inside `native_perform_command`).
struct GameBag<'a, 'g>(&'a mut Game<'g>);

impl Bag for GameBag<'_, '_> {
    fn held(&self) -> Option<(String, u32, bool)> {
        self.0.native_held()
    }
    fn carried(&self) -> Vec<(String, u32)> {
        self.0.native_carried_items()
    }
    fn count(&mut self, item: &str) -> u32 {
        self.0.native_count_items(item)
    }
    fn take(&mut self, item: &str, count: u32) -> u32 {
        self.0.native_take_items(item, count)
    }
    fn room(&mut self, item: &str) -> u32 {
        self.0.native_room_for(item)
    }
    fn give(&mut self, item: &str, count: u32) -> u32 {
        self.0.native_give_items(item, count)
    }
}

/// The most encoded data one listed stack may carry (a shulker box of
/// written books can hold megabytes; economy.json is written whole).
const MAX_STACK_DATA: usize = 32 * 1024;

/// A stack as the auction house keeps it: `ItemStack.CODEC`'s NBT (id,
/// count, components), as the game saves it in player files, in hex.
fn encode_stack(stack: &ItemStack) -> Option<String> {
    let tag = horizon_items::item_nbt::item_to_nbt(stack);
    let bytes = horizon_world::nbt::write_network(&tag, horizon_world::nbt::Limits::saved()).ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn decode_stack(data: &str) -> Option<ItemStack> {
    if data.len() % 2 != 0 || data.len() > 2 * MAX_STACK_DATA {
        return None;
    }
    let bytes: Vec<u8> = (0..data.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(data.get(i..i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    let (tag, _) = horizon_world::nbt::read_network(&bytes, horizon_world::nbt::Limits::default()).ok()?;
    horizon_items::item_nbt::item_from_nbt(&tag)
}

fn whole_of(stack: &ItemStack) -> Option<smp_auction::Whole> {
    Some(smp_auction::Whole { item: stack.item.to_string(), count: stack.count, data: encode_stack(stack)? })
}

/// Why `stack` can't be listed whole, if it can't: only what comes back
/// exactly as it went (every component read back the same) is taken.
fn unsellable(stack: &ItemStack) -> Option<String> {
    if &*stack.item == "minecraft:filled_map" && stack.data().and_then(|d| d.map_id).is_some_and(|m| m >= CHART_MAPS) {
        return Some("Market charts are free at /market: they can't be sold.".into());
    }
    let Some(data) = encode_stack(stack) else {
        return Some("That stack can't be listed: its data can't be written.".into());
    };
    if data.len() > 2 * MAX_STACK_DATA {
        return Some("That stack holds too much (books, contents...) to be listed.".into());
    }
    if decode_stack(&data).as_ref() != Some(stack) {
        return Some("That stack can't be listed whole: some of its data wouldn't come back the same.".into());
    }
    None
}

/// The same stack but for the count.
fn same_kind(a: &ItemStack, b: &ItemStack) -> bool {
    a.item == b.item && a.damage == b.damage && a.extra == b.extra
}

/// Whether `stack` fits whole in `slots` (the 36 main slots), as
/// `Inventory.add` places it: onto equal stacks, then into empty slots.
fn fits_in(slots: &[Option<ItemStack>], stack: &ItemStack, max: u32) -> bool {
    let room: u32 = slots
        .iter()
        .take(36)
        .map(|s| match s {
            None => max,
            Some(s) if same_kind(s, stack) => max.saturating_sub(s.count),
            Some(_) => 0,
        })
        .sum();
    room >= stack.count
}

/// `native_give_stack` for the auction house: what didn't fit, encoded
/// again (all of it when the data can't be read: it waits in the mail).
fn give_whole_to(game: &mut Game<'_>, uuid: u128, w: &smp_auction::Whole) -> Option<smp_auction::Whole> {
    let Some(stack) = decode_stack(&w.data) else {
        log::error!("[smp] auction house: a stack's data can't be read; it stays in the mail");
        return Some(w.clone());
    };
    let rest = game.native_give_stack(uuid, stack)?;
    Some(whole_of(&rest).unwrap_or_else(|| w.clone()))
}

impl smp_auction::Hand for GameBag<'_, '_> {
    /// The acting player's state is swapped into the game inside their
    /// command, so the hand is the game's selected slot (no helper hands out
    /// a whole stack yet).
    fn held_whole(&self) -> Option<Result<smp_auction::Whole, String>> {
        let s = self.0.inventory.get(self.0.selected)?.as_ref()?;
        if let Some(why) = unsellable(s) {
            return Some(Err(why));
        }
        whole_of(s).map(Ok)
    }
    fn take_held(&mut self, held: &smp_auction::Whole, count: u32) -> Option<smp_auction::Whole> {
        self.0.player.as_ref()?;
        let selected = self.0.selected;
        let slot = self.0.inventory.get_mut(selected)?;
        let s = slot.as_ref()?;
        if count == 0 || count > s.count || encode_stack(s).as_deref() != Some(held.data.as_str()) {
            return None;
        }
        let mut taken = s.clone();
        taken.count = count;
        let encoded = whole_of(&taken)?;
        if count == s.count {
            *slot = None;
        } else if let Some(s) = slot.as_mut() {
            s.count -= count;
        }
        Some(encoded)
    }
    fn fits(&mut self, w: &smp_auction::Whole) -> bool {
        let Some(stack) = decode_stack(&w.data) else {
            return false;
        };
        let max = self.0.native_max_stack_size(&stack.item).unwrap_or(64).max(1);
        fits_in(&self.0.inventory, &stack, max)
    }
    fn give_whole(&mut self, w: &smp_auction::Whole) -> Option<smp_auction::Whole> {
        let Some(me) = self.0.native_acting_player() else {
            return Some(w.clone());
        };
        give_whole_to(self.0, me.uuid, w)
    }
}

const COMMANDS: [&str; 16] = [
    "balance", "bal", "pay", "baltop", "eco", "money", "worth", "sell", "buy", "shop", "market", "trade", "coinflip",
    "lottery", "ah", "casino",
];

/// The world border, from the Overworld's `world_border.dat` (the root's in
/// older worlds), as the game last saved it; None without one (the default
/// border, 60 million blocks wide).
fn world_border(world: &std::path::Path) -> Option<smp_places::Bounds> {
    let files = [
        world.join("dimensions/minecraft/overworld/data/minecraft/world_border.dat"),
        world.join("data/minecraft/world_border.dat"),
    ];
    let root = files
        .iter()
        .find_map(|f| horizon_world::datafix::files::read_saved_dat(f).ok().flatten())?;
    let border = horizon_client::worldborder::WorldBorder::unpack(root.get("data")?);
    Some(border.bounds())
}

/// After a level's tick: in the Overworld, each `/rtp` that waits reads the
/// chunk it loads (the game reads only the level it ticks), five times a
/// second. Columns in another player's claim are never a spot.
pub(in crate::game) fn level_ticked(game: &mut Game<'_>, dimension: &str) {
    use std::sync::atomic::Ordering::Relaxed;
    if dimension != smp_places::OVERWORLD
        || !RTP.load(Relaxed)
        || TICKS.load(Relaxed) % 4 != 0
        || !game.native_on(ID)
    {
        return;
    }
    let Some(dir) = game.native_world_dir() else {
        return;
    };
    let mut guard = state(dir.join(smp_store::DIR));
    let Some(smp) = guard.as_mut() else {
        return;
    };
    let (Ok(places), claims) = (&mut smp.places, &smp.claims) else {
        return;
    };
    let game = &*game;
    let mut cost = RTP_COST.lock().unwrap_or_else(|e| e.into_inner());
    places.rtp_look(&mut |uuid, (cx, cz)| {
        let started = std::time::Instant::now();
        let ground = read_chunk(game, claims, uuid, (cx, cz));
        let us = started.elapsed().as_micros() as u64;
        match cost.iter_mut().find(|c| c.0 == uuid) {
            Some(c) => *c = (uuid, c.1 + 1, c.2 + us, c.3.max(us)),
            None => cost.push((uuid, 1, us, us)),
        }
        ground
    });
    RTP.store(places.rtp_searching(), Relaxed);
}

/// Chunk `(cx, cz)` of the Overworld as `/rtp` reads it, None while it isn't
/// loaded; no columns when another player claimed it.
fn read_chunk(
    game: &Game<'_>,
    claims: &Result<Claims, String>,
    uuid: u128,
    (cx, cz): (i32, i32),
) -> Option<smp_places::Ground> {
    let columns: Vec<(i32, i32)> = (0..256).map(|i| (cx * 16 + i / 16, cz * 16 + i % 16)).collect();
    let read = game
        .native_read_columns(
            smp_places::OVERWORLD,
            &columns,
            crate::game::native_world_edit::NativeHeightmap::MotionBlockingNoLeaves,
            (1, 2),
        )
        .ok()?;
    if read.bases.iter().any(Option::is_none) {
        return None;
    }
    let mut ground = smp_places::Ground { palette: read.palette, columns: Vec::new() };
    let claimed = claims
        .as_ref()
        .ok()
        .and_then(|c| c.owner(smp_places::OVERWORLD, cx * 16, cz * 16))
        .is_some_and(|owner| owner != uuid);
    if !claimed {
        for (i, (&(x, z), base)) in columns.iter().zip(&read.bases).enumerate() {
            let b = &read.blocks[i * 3..i * 3 + 3];
            ground.columns.push((x, z, base.unwrap_or_default(), [b[0], b[1], b[2]]));
        }
    }
    Some(ground)
}

/// The players on, as the places see them.
fn bodies(players: &[NativeServerPlayer]) -> Vec<Body> {
    players
        .iter()
        .map(|p| Body {
            uuid: p.uuid,
            name: p.name.clone(),
            at: Place {
                dimension: p.dimension.clone(),
                x: p.position[0],
                y: p.position[1],
                z: p.position[2],
            },
            health: p.health,
        })
        .collect()
}

/// A random number below `n` (n > 0), never the world's random: a fresh
/// SipHash key (seeded from the system's randomness) over the time.
fn below(n: u64) -> u64 {
    use std::hash::BuildHasher;
    let time = std::time::SystemTime::now();
    std::collections::hash_map::RandomState::new().hash_one(time) % n.max(1)
}

/// A fair coin, never the world's random: a fresh SipHash key (seeded from
/// the system's randomness) over the time.
fn heads() -> bool {
    use std::hash::BuildHasher;
    let time = std::time::SystemTime::now();
    std::collections::hash_map::RandomState::new().hash_one(time) & 1 == 1
}

/// A player's command.
pub(in crate::game) fn perform(
    game: &mut Game<'_>,
    player: &NativeServerPlayer,
    line: &str,
    ran: &mut bool,
) {
    let word = line.split(' ').next().unwrap_or_default();
    let ours = [
        &COMMANDS[..],
        &smp_places::COMMANDS,
        &smp_claims::COMMANDS,
        &smp_board::COMMANDS,
        &smp_presence::COMMANDS,
        &smp_nick::COMMANDS,
        &smp_vanish::COMMANDS,
        &smp_switch::COMMANDS,
        &smp_reports::COMMANDS,
        &smp_ledger::COMMANDS,
        &smp_admin::COMMANDS,
    ];
    if !game.native_on(ID) || !ours.iter().any(|c| c.contains(&word)) {
        return;
    }
    BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
    let Some(dir) = game.native_world_dir() else {
        return;
    };
    let me = Player {
        uuid: player.uuid,
        name: player.name.clone(),
        level: player.permission,
    };
    // Survival and adventure: a creative player could sell what they made.
    let trades = matches!(player.game_mode, 0 | 2);
    let players = game.native_server_players();
    let now = chrono::Utc::now().timestamp_millis();
    let mut guard = state(dir.join(smp_store::DIR));
    let smp = guard.as_mut().expect("opened");
    // A vanished player is out of reach (trades, teleports, bets) of those
    // who don't see them.
    let hidden: Vec<u128> = smp.vanish.as_ref().map_or_else(
        |_| Vec::new(),
        |v| players.iter().filter(|p| !v.sees(me.level, p.uuid)).map(|p| p.uuid).collect(),
    );
    let online = |name: &str| {
        players
            .iter()
            .filter(|p| !hidden.contains(&p.uuid))
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| (p.uuid, p.name.clone(), matches!(p.game_mode, 0 | 2)))
    };
    // The switches: `/smp` itself, then a part turned off answers alone.
    if let Some((out, changed)) = smp.switches.command(&me, now, line) {
        let out = switched(game, smp, &me, &players, changed, now, out);
        drop(guard);
        *ran = true;
        show(game, out);
        return;
    }
    if let Some(part) = smp_switch::Part::of_command(word).filter(|&p| !smp.switches.on(p)) {
        let refusal = smp_switch::refusal(part);
        drop(guard);
        *ran = true;
        show(game, vec![refusal]);
        return;
    }
    let mut acts = Vec::new();
    // Any SMP command is activity (no longer AFK), and past the limit waits.
    let mut sleeps = Vec::new();
    let mut before = Vec::new();
    if let Ok(p) = &mut smp.presence
        && smp.switches.on(Part::Presence)
    {
        match p.allow(&me, now) {
            Ok((o, a)) => (before, sleeps) = (o, a),
            Err(o) => {
                drop(guard);
                *ran = true;
                show(game, vec![o]);
                return;
            }
        }
    }
    // A nickname where a command names a player: the player's name.
    let names_a_player = matches!(word, "pay" | "tpa" | "trade" | "playtime" | "tpaccept" | "tpdeny" | "eco" | "money");
    let resolved = smp.nicks.as_ref().ok().filter(|_| names_a_player).and_then(|n| {
        let accounts = smp.economy.as_ref().map(|e| e.accounts()).unwrap_or_default();
        let is_name = |w: &str| players.iter().any(|p| p.name.eq_ignore_ascii_case(w)) || accounts.iter().any(|a| a.1.eq_ignore_ascii_case(w));
        let name_of = |u: u128| {
            players.iter().find(|p| p.uuid == u).map(|p| p.name.clone()).or_else(|| accounts.iter().find(|a| a.0 == u).map(|a| a.1.clone()))
        };
        n.resolve(line, &is_name, &name_of)
    });
    let line = resolved.as_deref().unwrap_or(line);
    let mut admin = Vec::new();
    let out = if smp_admin::COMMANDS.contains(&word) {
        let economy = smp.economy.as_ref().ok();
        let names = ServerNames::read(&dir, economy);
        let find = |n: &str| {
            let on = players.iter().find(|p| p.name.eq_ignore_ascii_case(n));
            on.map(|p| (p.uuid, p.name.clone())).or_else(|| names.find(n))
        };
        let name_of = |u: u128| {
            let on = players.iter().find(|p| p.uuid == u).map(|p| p.name.clone());
            on.or_else(|| names.name_of(u)).unwrap_or_else(|| smp_store::uuid_string(u))
        };
        let online = |u: u128| bodies(&players).into_iter().find(|b| b.uuid == u).map(|b| b.at);
        let off = |what: &str| format!("{what} can't be read (see the server log)");
        let look = smp_admin::Lookup {
            claims: smp.claims.as_ref().map_err(|_| off("claims.json")),
            places: smp.places.as_ref().map_err(|_| off("places.json")),
            last: smp.last.as_ref().map_err(|_| off("last_positions.json")),
            find: &find,
            name_of: &name_of,
            online: &online,
        };
        admin = smp_admin::command(&me, &look, now, line).unwrap_or_default();
        Vec::new()
    } else if smp_reports::COMMANDS.contains(&word) {
        let staff: Vec<u128> = players.iter().filter(|p| p.permission >= smp_reports::LEVEL).map(|p| p.uuid).collect();
        let economy = smp.economy.as_ref().ok();
        let find = |n: &str| {
            let on = players.iter().find(|p| p.name.eq_ignore_ascii_case(n));
            on.map(|p| (p.uuid, p.name.clone())).or_else(|| ServerNames::read(&dir, economy).find(n))
        };
        let at = |u: u128| {
            let p = players.iter().find(|p| p.uuid == u)?;
            let [x, y, z] = p.position.map(|v| v.floor() as i64);
            Some(format!("{} {x} {y} {z}", p.dimension))
        };
        let out = smp.reports.command(&me, &find, &at, &staff, now, line).unwrap_or_default();
        for l in smp.reports.logged.drain(..) {
            log::info!("[smp] {l}");
        }
        out
    } else if smp_ledger::COMMANDS.contains(&word) {
        let economy = smp.economy.as_ref().ok();
        let find = |n: &str| {
            let on = players.iter().find(|p| p.name.eq_ignore_ascii_case(n));
            on.map(|p| (p.uuid, p.name.clone())).or_else(|| economy?.find(n).map(|(u, a)| (u, a.name.clone())))
        };
        let name_of = |u: u128| economy?.name_of(u).map(str::to_owned);
        let money = |n: i64| economy.map_or_else(|| n.to_string(), |e| e.money(n));
        smp_ledger::command(&me, &smp.dir.join("audit.log"), &find, &name_of, &money, now, line).unwrap_or_default()
    } else if smp_vanish::COMMANDS.contains(&word) {
        let ops: Vec<u128> = players.iter().filter(|p| p.permission >= smp_vanish::LEVEL).map(|p| p.uuid).collect();
        match &mut smp.vanish {
            Ok(v) => {
                let out = v.command(&me, &ops, line).unwrap_or_default();
                // Hidden or shown at once; the subtitle comes with the next
                // second (commands can't run inside a player's).
                for (uuid, hidden) in v.changes() {
                    game.native_hide_player(uuid, hidden.then_some(smp_vanish::LEVEL));
                }
                if let Ok(p) = &mut smp.presence {
                    p.quiet = v.quiet();
                }
                out
            }
            Err(_) => vec![Out::Error("/vanish is off: vanish.json can't be read (see the server log).".into())],
        }
    } else if smp_nick::COMMANDS.contains(&word) {
        let names = ServerNames::read(&dir, smp.economy.as_ref().ok());
        match &mut smp.nicks {
            Ok(n) => {
                let played = smp.places.as_ref().map_or(0, |p| p.played_seconds(me.uuid));
                n.command(&me, &names, played, now, line).unwrap_or_default()
            }
            Err(_) => vec![Out::Error("Nicknames are off: nicks.json can't be read (see the server log).".into())],
        }
    } else if smp_presence::COMMANDS.contains(&word) {
        match &mut smp.presence {
            Ok(p) => match p.command(&me, line) {
                Some((o, a)) => {
                    sleeps.extend(a);
                    o
                }
                None => Vec::new(),
            },
            Err(_) => vec![Out::Error(
                "AFK is off: presence.json can't be read (see the server log).".into(),
            )],
        }
    } else if smp_board::COMMANDS.contains(&word) {
        match &mut smp.board {
            Ok(b) => {
                let (out, show) = b.command(&me, line).unwrap_or_default();
                // Shown again at the next second's update; taken away now.
                if show == Some(false) {
                    game.native_sidebar(me.uuid, None);
                }
                out
            }
            Err(_) => vec![Out::Error(
                "The sidebar is off: its file can't be read (see the server log).".into(),
            )],
        }
    } else if line.split_whitespace().eq(["claim", "show"]) {
        match bodies(&players).into_iter().find(|b| b.uuid == me.uuid) {
            Some(b) => {
                let (cx, cz) = ((b.at.x.floor() as i32) >> 4, (b.at.z.floor() as i32) >> 4);
                smp.shows.insert(me.uuid, (b.at.dimension, cx, cz, now + 10_000));
                vec![Out::Reply(
                    "This chunk's border, for 10 s: green yours, red someone else's, white free.".into(),
                )]
            }
            None => vec![Out::Error("You are not in the world.".into())],
        }
    } else if smp_claims::COMMANDS.contains(&word) {
        let at = bodies(&players).into_iter().find(|b| b.uuid == me.uuid).map(|b| b.at);
        let played = smp.places.as_ref().map_or(0, |p| p.played_seconds(me.uuid));
        let economy = smp.economy.as_ref().ok();
        let find = |n: &str| {
            let on = players.iter().find(|p| p.name.eq_ignore_ascii_case(n));
            on.map(|p| (p.uuid, p.name.clone()))
                .or_else(|| economy?.find(n).map(|(u, a)| (u, a.name.clone())))
        };
        let name_of = |u: u128| {
            let on = players.iter().find(|p| p.uuid == u).map(|p| p.name.clone());
            on.or_else(|| economy?.name_of(u).map(str::to_owned))
                .unwrap_or_else(|| smp_store::uuid_string(u))
        };
        match (&mut smp.claims, at) {
            (Ok(c), Some(at)) => c.command(&me, &at, played, &find, &name_of, line).unwrap_or_default(),
            (Ok(_), None) => vec![Out::Error("You are not in the world.".into())],
            (Err(_), _) => vec![Out::Error(
                "Claims are off: their file can't be read (see the server log).".into(),
            )],
        }
    } else if smp_places::COMMANDS.contains(&word) {
        match &mut smp.places {
            Ok(p) => {
                if matches!(word, "rtp" | "wild") {
                    p.border = world_border(&dir);
                    RTP_COST.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| c.0 != me.uuid);
                    BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let seen: Vec<NativeServerPlayer> = players.iter().filter(|p| !hidden.contains(&p.uuid)).cloned().collect();
                p.command(&me, &bodies(&seen), now, line).unwrap_or_default()
            }
            Err(_) => vec![Out::Error(
                "Homes and warps are off: their file can't be read (see the server log).".into(),
            )],
        }
    } else if word == "shop" {
        open_shop(game, smp, &me)
    } else if word == "market" {
        open_market(game, smp, &me, line, now)
    } else if word == "ah" {
        ah_command(game, smp, &me, trades, now, line)
    } else if word == "casino" {
        match &mut smp.economy {
            Ok(e) => {
                e.seen(me.uuid, &me.name);
                smp_casino_glue::command(game, &mut smp.casino, e, &me, now, line)
            }
            Err(_) => vec![Out::Error("The economy is off: its file can't be read (see the server log).".into())],
        }
    } else {
        match &mut smp.economy {
        Ok(e) => {
            e.seen(me.uuid, &me.name);
            let on: Vec<(u128, String)> = players.iter().map(|p| (p.uuid, p.name.clone())).collect();
            let mut bag = GameBag(game);
            let report = smp.report.as_ref();
            report
                .and_then(|r| r.command(e, line))
                .or_else(|| e.command(&me, &on, line))
                .or_else(|| smp.market.command(e, &me, trades, &mut bag, line))
                .or_else(|| {
                    let (out, a) = smp.trades.command(e, &me, trades, &online, now, line)?;
                    acts = a;
                    Some(out)
                })
                .or_else(|| {
                    let named = |n: &str| online(n).map(|(u, name, _)| (u, name));
                    smp.games.command(e, &me, &named, now, &mut heads, line)
                })
                .or_else(|| smp.lottery.command(&smp.games.config, e, &me, now, line))
                .unwrap_or_default()
        }
        Err(_) => vec![Out::Error(
            "The economy is off: its file can't be read (see the server log).".into(),
        )],
        }
    };
    act(game, smp, acts);
    drop(guard);
    *ran = true;
    // Inside the player's command: the sleep count changes at the next
    // second's tick (`native_sleep_ignored` works outside a player's hook).
    if !sleeps.is_empty() {
        PENDING_SLEEP.lock().unwrap_or_else(|e| e.into_inner()).extend(sleeps);
    }
    before.extend(out);
    show(game, before);
    show_admin(game, admin);
}

/// `/smpadmin`'s lines, each place a link: a click teleports the operator
/// there (or, for a claim, whose ground isn't known, puts the command in
/// their chat box).
fn show_admin(game: &mut Game<'_>, lines: Vec<smp_admin::Line>) {
    use horizon_client::text::{ClickEvent, Style};
    for l in lines {
        let color = if l.error { text::RED } else if l.go.is_some() { text::GREEN } else { text::GRAY };
        let mut t = Text::literal(l.text).with_color(color);
        if let Some(go) = l.go {
            let (click, hover) = match &go {
                smp_admin::Go::Exact(_) => (ClickEvent::RunCommand(go.command()), "Click to teleport there"),
                smp_admin::Go::Column { .. } => (ClickEvent::SuggestCommand(go.command()), "Click to put the teleport in your chat box"),
            };
            t = t.with_style(&Style {
                click: Some(click),
                hover: Some(Box::new(Text::literal(hover))),
                ..Style::default()
            });
        }
        game.native_feedback(t);
    }
}

/// A player left the game (`native_player_logged_out`): where they stood is
/// kept for `/smpadmin lastpos`. When the server stops they are still
/// listed, so that place is exact; otherwise it is where the last second's
/// look saw them. Queued for the next tick (or the save as the world
/// closes): the hook may come while the SMP's state is held.
pub(in crate::game) fn player_left(game: &mut Game<'_>, uuid: u128, name: &str) {
    if !game.native_on(ID) {
        return;
    }
    let listed = game.native_server_players().into_iter().find(|p| p.uuid == uuid);
    let at = listed.map(|p| bodies(&[p]).remove(0).at);
    let now = chrono::Utc::now().timestamp_millis();
    LEFT.lock().unwrap_or_else(|e| e.into_inner()).push((uuid, name.to_owned(), at, now));
    BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The players who left since the last look, into `last_positions.json`.
fn note_leavers(smp: &mut Smp) {
    let left = std::mem::take(&mut *LEFT.lock().unwrap_or_else(|e| e.into_inner()));
    for (uuid, name, at, when) in left {
        let stood = smp.standing.remove(&uuid);
        if let (Ok(l), Some(at)) = (&mut smp.last, at.or(stood)) {
            l.left(uuid, &name, &at, when);
        }
    }
}

/// A player breaks or right clicks the block at `pos`: canceled in a chunk
/// claimed by someone who doesn't trust them (`native_destroy_block`,
/// `native_right_click_block`).
pub(in crate::game) fn guard(game: &mut Game<'_>, pos: [i32; 3], canceled: &mut bool) {
    if *canceled || !game.native_on(ID) {
        return;
    }
    let (Some(who), Some(dir)) = (game.native_acting_player(), game.native_world_dir()) else {
        return;
    };
    let mut guard = state(dir.join(smp_store::DIR));
    let smp = guard.as_mut().expect("opened");
    if !smp.switches.on(Part::Claims) {
        return;
    }
    let Ok(claims) = &mut smp.claims else {
        return;
    };
    let me = Player {
        uuid: who.uuid,
        name: who.name.clone(),
        level: who.permission,
    };
    let economy = smp.economy.as_ref().ok();
    let name_of = |u: u128| {
        economy
            .and_then(|e| e.name_of(u))
            .map_or_else(|| smp_store::uuid_string(u), str::to_owned)
    };
    let now = chrono::Utc::now().timestamp_millis();
    if let Err(line) = claims.check(&me, &who.dimension, pos, &name_of, now) {
        *canceled = true;
        drop(guard);
        if let Some(line) = line {
            game.native_action_bar(who.uuid, Text::literal(line).with_color(text::RED));
        }
    }
}
/// A player hits the entity of type `kind` at `pos`: canceled when claims
/// protect that type and the chunk is claimed by someone who doesn't trust
/// them (`native_attack_entity`).
pub(in crate::game) fn attack(game: &mut Game<'_>, kind: &str, pos: [f64; 3], canceled: &mut bool) {
    if *canceled || !game.native_on(ID) {
        return;
    }
    let protected = STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|s| s.switches.on(Part::Claims) && s.claims.as_ref().is_ok_and(|c| c.protects(kind)));
    if protected {
        guard(game, pos.map(|v| v.floor() as i32), canceled);
    }
}

/// The world's dimension id, as players' places name it.
pub(in crate::game) fn dimension_id(dimension: crate::game::LevelDimension) -> &'static str {
    match dimension {
        crate::game::LevelDimension::Overworld => "minecraft:overworld",
        crate::game::LevelDimension::Nether => "minecraft:the_nether",
        crate::game::LevelDimension::End => "minecraft:the_end",
    }
}

/// Whether the world may change the blocks `to` from `from` (a fluid, fire,
/// a piston's blocks): not into a claim from land of another owner. Asked
/// on every flow: it only reads the claims already open, and allows while
/// the SMP's state is busy (a hook inside the SMP's own call).
pub(in crate::game) fn world_allows(dimension: &str, from: [i32; 3], to: &[[i32; 3]]) -> bool {
    let Ok(state) = STATE.try_lock() else {
        return true;
    };
    match state.as_ref().filter(|s| s.switches.on(Part::Claims)).map(|s| &s.claims) {
        Some(Ok(c)) => to.iter().all(|&t| c.spread_allowed(dimension, from, t)),
        _ => true,
    }
}

/// An explosion is about to break `blocks`: claimed land is spared
/// (`native_explosion_detonate`).
pub(in crate::game) fn explosion(game: &Game<'_>, dimension: &str, blocks: &mut Vec<[i32; 3]>) {
    if !game.native_on(ID) {
        return;
    }
    let Ok(state) = STATE.try_lock() else {
        return;
    };
    if let Some(Ok(c)) = state.as_ref().filter(|s| s.switches.on(Part::Claims)).map(|s| &s.claims) {
        c.spare(dimension, blocks);
    }
}

/// `f` on the open claims, when claims are on: None while the SMP's state
/// is busy (a hook inside the SMP's own call), before it opened, or with
/// claims off (`smp_claims_glue`).
pub(in crate::game) fn with_claims<R>(f: impl FnOnce(&Claims) -> R) -> Option<R> {
    let state = STATE.try_lock().ok()?;
    let smp = state.as_ref().filter(|s| s.switches.on(Part::Claims))?;
    smp.claims.as_ref().ok().map(f)
}

/// Whether the SMP runs in this world.
pub(in crate::game) fn claims_live(game: &Game<'_>) -> bool {
    game.native_on(ID)
}

/// The server ticks counted so far: the same number all through one tick.
pub(in crate::game) fn ticks() -> u64 {
    TICKS.load(std::sync::atomic::Ordering::Relaxed)
}

/// An item named `name` (not in italics, as a name the game gives).
fn button(item: &str, name: &str, color: &str) -> ItemStack {
    let mut stack = ItemStack {
        item: item.into(),
        count: 1,
        damage: 0,
        extra: None,
    };
    let title = serde_json::json!({ "text": name, "italic": false, "color": color });
    stack.data_mut().custom_name = Some(component_nbt(&title));
    stack
}

/// The trade menu's cells as items.
fn menu_items(smp: &Smp, uuid: u128) -> Vec<Option<ItemStack>> {
    let money = |n| match &smp.economy {
        Ok(e) => e.money(n),
        Err(_) => n.to_string(),
    };
    smp.trades
        .layout(uuid)
        .into_iter()
        .map(|cell| match cell {
            Cell::Empty => None,
            Cell::Stack(s) => Some(s),
            Cell::Divider => Some(button("minecraft:gray_stained_glass_pane", " ", "gray")),
            Cell::Money(true, n) => Some(button(
                "minecraft:gold_ingot",
                &format!("Your money: {} (/trade money <amount>)", money(n)),
                "gold",
            )),
            Cell::Money(false, n) => Some(button(
                "minecraft:gold_ingot",
                &format!("Their money: {}", money(n)),
                "gold",
            )),
            Cell::Confirm(false) => Some(button(
                "minecraft:lime_stained_glass_pane",
                "Confirm the trade",
                "green",
            )),
            Cell::Confirm(true) => Some(button(
                "minecraft:lime_concrete",
                "Confirmed (click to take it back)",
                "green",
            )),
            Cell::Cancel => Some(button(
                "minecraft:red_stained_glass_pane",
                "Cancel the trade",
                "red",
            )),
            Cell::Other(name, true) => Some(button(
                "minecraft:lime_concrete",
                &format!("{name} confirmed"),
                "green",
            )),
            Cell::Other(name, false) => Some(button(
                "minecraft:gray_concrete",
                &format!("{name} has not confirmed"),
                "gray",
            )),
        })
        .collect()
}

/// Carries out what the trade rules ask.
fn act(game: &mut Game<'_>, smp: &mut Smp, acts: Vec<Act>) {
    for a in acts {
        match a {
            Act::Show(uuid) => {
                let items = menu_items(smp, uuid);
                let Some(t) = smp.trades.open.iter_mut().find(|t| t.side_of(uuid).is_some()) else {
                    continue;
                };
                let me = t.side_of(uuid).expect("found");
                if t.sides[me].window.is_some() {
                    game.native_set_menu_items(uuid, items);
                } else {
                    let title = Text::literal(format!("Trade with {}", t.sides[1 - me].name));
                    t.sides[me].window = game.native_open_menu(uuid, smp_trade::ROWS, title, items);
                }
            }
            Act::Close(uuid) => game.native_close_menu(uuid),
            Act::Settle(uuid) => settle(game, smp, uuid),
        }
    }
}

/// Both confirmed: the trade is logged, then both offers swap in one step
/// with the money, or nothing moves and both are told why.
fn settle(game: &mut Game<'_>, smp: &mut Smp, uuid: u128) {
    let Some(t) = smp.trades.trade_of(uuid).cloned() else {
        return;
    };
    let [a, b] = &t.sides;
    let players = game.native_server_players();
    let may = |u: u128| players.iter().any(|p| p.uuid == u && matches!(p.game_mode, 0 | 2));
    let Ok(economy) = &mut smp.economy else {
        return;
    };
    let fail = |why: String| -> Vec<Out> {
        t.sides
            .iter()
            .map(|s| Out::Tell(s.uuid, format!("The trade didn't go through: {why}.")))
            .collect()
    };
    let stacks = |side: &smp_trade::Side<ItemStack>| -> serde_json::Value {
        side.offer
            .iter()
            .map(|(_, s)| serde_json::json!({ "item": &*s.item, "count": s.count, "plain": s.damage == 0 && s.extra.is_none() }))
            .collect()
    };
    let out = if !may(a.uuid) || !may(b.uuid) {
        fail("both must be here, in survival or adventure mode".into())
    } else if economy.balance(a.uuid) < a.money || economy.balance(b.uuid) < b.money {
        fail("one of you no longer has the money offered".into())
    } else if let Err(e) = economy.record(serde_json::json!({
        "kind": "trade",
        "a": smp_store::uuid_string(a.uuid), "a_items": stacks(a), "a_money": a.money,
        "b": smp_store::uuid_string(b.uuid), "b_items": stacks(b), "b_money": b.money,
    })) {
        fail(e)
    } else if let Err(e) = game.native_exchange_items(a.uuid, &a.offer, b.uuid, &b.offer) {
        let _ = economy.record(serde_json::json!({ "kind": "trade_failed", "why": &e }));
        fail(e)
    } else {
        // The balances were checked above: only the log could refuse now.
        let net = a.money - b.money;
        let paid = match net.signum() {
            1 => economy.transfer("trade_money", a.uuid, b.uuid, net),
            -1 => economy.transfer("trade_money", b.uuid, a.uuid, -net),
            _ => Ok(()),
        };
        if let Err(e) = paid {
            log::error!("[smp] a trade's money: {e}");
        }
        let (_, acts) = smp.trades.end(uuid, "");
        // Ended as a success: the menus close, with their own line.
        for act in acts {
            if let Act::Close(u) = act {
                game.native_close_menu(u);
            }
        }
        let line = |me: &smp_trade::Side<ItemStack>, them: &smp_trade::Side<ItemStack>| {
            Out::Tell(
                me.uuid,
                format!("Trade with {} done: you gave {} stack(s), got {}.", them.name, me.offer.len(), them.offer.len()),
            )
        };
        show(game, vec![line(a, b), line(b, a)]);
        return;
    };
    // Not settled: both must confirm again.
    if let Some(t) = smp.trades.open.iter_mut().find(|t| t.side_of(uuid).is_some()) {
        for s in &mut t.sides {
            s.confirmed = false;
        }
    }
    act(game, smp, vec![Act::Show(a.uuid), Act::Show(b.uuid)]);
    show(game, out);
}

/// A click in a native menu: a trade's, when it is one.
pub(in crate::game) fn menu_clicked(game: &mut Game<'_>, uuid: u128, window: i32, slot: i32, mode: i32) {
    // A plain click or a shift-click; drags and double clicks are ignored.
    if !game.native_on(ID) || !matches!(mode, 0 | 1) {
        return;
    }
    let Some(dir) = game.native_world_dir() else {
        return;
    };
    let mut guard = state(dir.join(smp_store::DIR));
    let smp = guard.as_mut().expect("opened");
    // A menu of a part turned off since it opened: closed, nothing done.
    if let Some(part) = menu_part(smp, uuid, window).filter(|&p| !smp.switches.on(p)) {
        let refusal = smp_switch::refusal(part);
        drop(guard);
        game.native_close_menu(uuid);
        tell(game, uuid, vec![refusal]);
        return;
    }
    if smp.shop.views.get(&uuid).is_some_and(|v| v.0 == Some(window)) {
        shop_clicked(game, smp, uuid, slot, mode == 1);
        return;
    }
    if smp.market_menus.views.get(&uuid).is_some_and(|v| v.0 == Some(window)) {
        market_clicked(game, smp, uuid, slot);
        return;
    }
    if smp.auction.as_ref().is_some_and(|a| a.views.get(&uuid).is_some_and(|v| v.0 == Some(window))) {
        ah_clicked(game, smp, uuid, slot);
        return;
    }
    if smp.casino.owns(uuid, window) {
        let Ok(economy) = &mut smp.economy else {
            return;
        };
        let commands = smp_casino_glue::clicked(game, &mut smp.casino, economy, uuid, slot, mode == 1);
        if smp.casino.animating() {
            BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        drop(guard);
        for line in commands {
            game.native_run_command(&line);
        }
        return;
    }
    let Some(t) = smp.trades.trade_of(uuid) else {
        return;
    };
    let me = t.side_of(uuid).expect("found");
    if t.sides[me].window != Some(window) {
        return;
    }
    let (out, acts) = match native_menu_inventory_slot(smp_trade::SIZE, slot) {
        Some(i) => match game.native_player_stacks(uuid).and_then(|s| s.get(i).cloned().flatten()) {
            Some(stack) => smp.trades.click(uuid, None, Some((i, stack))),
            None => (Vec::new(), Vec::new()),
        },
        None => match usize::try_from(slot).ok().filter(|&s| s < smp_trade::SIZE) {
            Some(s) => smp.trades.click(uuid, Some(s), None),
            None => (Vec::new(), Vec::new()),
        },
    };
    act(game, smp, acts);
    drop(guard);
    show(game, out);
}

/// The part whose menu `window` is, open for that player.
fn menu_part(smp: &Smp, uuid: u128, window: i32) -> Option<Part> {
    let open = |v: Option<i32>| v == Some(window);
    if smp.shop.views.get(&uuid).is_some_and(|v| open(v.0)) || smp.market_menus.views.get(&uuid).is_some_and(|v| open(v.0)) {
        return Some(Part::Market);
    }
    if smp.auction.as_ref().is_some_and(|a| a.views.get(&uuid).is_some_and(|v| open(v.0))) {
        return Some(Part::Auction);
    }
    if smp.casino.owns(uuid, window) {
        return Some(Part::Gambling);
    }
    let trade = smp.trades.trade_of(uuid);
    trade.and_then(|t| t.side_of(uuid).map(|me| t.sides[me].window)).filter(|&w| open(w)).map(|_| Part::Trades)
}

/// Parts turned off or on by `/smp`: what was under way in those turned off
/// stops (trades end with nothing moved, menus close, teleports waiting are
/// called off, sidebars and nicknames are taken away), the other operators
/// on are told, and the change is in the server's log and `audit.log`.
fn switched(
    game: &mut Game<'_>,
    smp: &mut Smp,
    me: &Player,
    players: &[NativeServerPlayer],
    changed: smp_switch::Changed,
    now: i64,
    mut out: Vec<Out>,
) -> Vec<Out> {
    let names = |ps: &[Part]| ps.iter().map(|p| p.name()).collect::<Vec<_>>().join(", ");
    let reason = line_reason(&smp.switches, &changed);
    for (verb, parts) in [("off", &changed.off), ("on", &changed.on)] {
        if parts.is_empty() {
            continue;
        }
        log::warn!("[smp] {} turned {verb}: {}{reason}", me.name, names(parts));
        if let Ok(e) = &mut smp.economy
            && let Err(err) = e.record(serde_json::json!({
                "time": now, "kind": format!("switch_{verb}"), "by": smp_store::uuid_string(me.uuid),
                "parts": parts.iter().map(|p| p.name()).collect::<Vec<_>>(), "reason": reason.trim_start_matches(": "),
            }))
        {
            log::error!("[smp] {err}");
        }
        for p in players.iter().filter(|p| p.permission >= smp_switch::LEVEL && p.uuid != me.uuid) {
            out.push(Out::Tell(p.uuid, format!("[SMP] {} turned {verb}: {}{reason}", me.name, names(parts))));
        }
    }
    for part in changed.off {
        match part {
            Part::Trades => {
                let sides: Vec<u128> = smp.trades.open.iter().map(|t| t.sides[0].uuid).collect();
                for uuid in sides {
                    let (o, acts) = smp.trades.end(uuid, "trading was turned off");
                    out.extend(o);
                    act(game, smp, acts);
                }
            }
            Part::Market => {
                let open: Vec<u128> = smp.shop.views.keys().chain(smp.market_menus.views.keys()).copied().collect();
                open.into_iter().for_each(|u| game.native_close_menu(u));
            }
            Part::Auction => {
                let open: Vec<u128> = smp.auction.as_ref().map(|a| a.views.keys().copied().collect()).unwrap_or_default();
                open.into_iter().for_each(|u| game.native_close_menu(u));
            }
            // A round under way still ends and is paid when its menu closes
            // (`menu_clicked` closes it at the next click).
            Part::Gambling => {}
            Part::Places => {
                if let Ok(p) = &mut smp.places {
                    for t in std::mem::take(&mut p.pending) {
                        out.push(Out::Tell(t.uuid, "Teleport called off: teleporting was turned off.".into()));
                    }
                }
            }
            Part::Claims => smp.shows.clear(),
            Part::Board => {
                if let Ok(b) = &mut smp.board {
                    b.forget();
                }
                let blank = Text::literal("");
                for p in players {
                    game.native_sidebar(p.uuid, None);
                    game.native_tab_list(p.uuid, &blank, &blank);
                }
            }
            Part::Nicks => {
                for uuid in std::mem::take(&mut smp.nicks_shown) {
                    game.native_set_nickname(uuid, None);
                }
            }
            // `/invsee` refuses and its open menus take no click.
            Part::Invsee | Part::Economy | Part::Presence => {}
        }
    }
    BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
    out
}

/// `: why` of the parts just turned off (as `switches.json` has it), or "".
fn line_reason(switches: &Switches, changed: &smp_switch::Changed) -> String {
    changed
        .off
        .first()
        .and_then(|&p| switches.why(p))
        .filter(|r| !r.is_empty())
        .map(|r| format!(": {r}"))
        .unwrap_or_default()
}

/// What a command of a part turned off answers.
pub(in crate::game) fn refusal(part: Part) -> String {
    match smp_switch::refusal(part) {
        Out::Error(s) | Out::Reply(s) | Out::Tell(_, s) | Out::All(s) => s,
    }
}

/// Whether a part of the SMP is on (`/smp off` turns them off), for the
/// SMP's other glue (`/invsee`).
pub(in crate::game) fn part_on(game: &Game<'_>, part: Part) -> bool {
    let Some(dir) = game.native_world_dir() else {
        return true;
    };
    state(dir.join(smp_store::DIR)).as_ref().is_none_or(|s| s.switches.on(part))
}

/// A native menu closed: a trade's ends, nothing moved.
pub(in crate::game) fn menu_closed(game: &mut Game<'_>, uuid: u128, window: i32) {
    if !game.native_on(ID) {
        return;
    }
    let Some(dir) = game.native_world_dir() else {
        return;
    };
    let mut guard = state(dir.join(smp_store::DIR));
    let smp = guard.as_mut().expect("opened");
    if smp.shop.views.get(&uuid).is_some_and(|v| v.0 == Some(window)) {
        smp.shop.views.remove(&uuid);
        return;
    }
    if smp.market_menus.views.get(&uuid).is_some_and(|v| v.0 == Some(window)) {
        smp.market_menus.views.remove(&uuid);
        return;
    }
    if let Some(a) = &mut smp.auction
        && a.views.get(&uuid).is_some_and(|v| v.0 == Some(window))
    {
        a.views.remove(&uuid);
        return;
    }
    // A round under way carries on and is paid all the same.
    if smp.casino.closed(uuid, window, chrono::Utc::now().timestamp_millis()) {
        return;
    }
    let Some(t) = smp.trades.trade_of(uuid) else {
        return;
    };
    let me = t.side_of(uuid).expect("found");
    if t.sides[me].window != Some(window) {
        return;
    }
    let why = format!("{} closed the trade", t.sides[me].name);
    let (out, acts) = smp.trades.end(uuid, &why);
    act(game, smp, acts);
    drop(guard);
    show(game, out);
}

/// A player's items outside their command (a click in a menu): read from
/// their inventory, moved by the console's `clear` and `give`, each move
/// measured on the inventory before and after.
struct MenuBag<'a, 'g> {
    game: &'a mut Game<'g>,
    uuid: u128,
    name: String,
}

impl MenuBag<'_, '_> {
    fn stacks(&self) -> Vec<Option<ItemStack>> {
        self.game.native_player_stacks(self.uuid).unwrap_or_default()
    }
    /// Every stack of `item`, plain or not (`clear` takes any).
    fn total(&self, item: &str) -> u32 {
        self.stacks().iter().flatten().filter(|s| &*s.item == item).map(|s| s.count).sum()
    }
}

fn plain(s: &ItemStack) -> bool {
    s.damage == 0 && s.extra.is_none()
}

impl Bag for MenuBag<'_, '_> {
    fn held(&self) -> Option<(String, u32, bool)> {
        None
    }
    fn carried(&self) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = Vec::new();
        for s in self.stacks().iter().flatten().filter(|s| plain(s)) {
            match out.iter_mut().find(|(id, _)| **id == *s.item) {
                Some((_, n)) => *n += s.count,
                None => out.push((s.item.to_string(), s.count)),
            }
        }
        out
    }
    /// The plain ones; none while a named or worn one is carried, which
    /// `clear` could take in their place.
    fn count(&mut self, item: &str) -> u32 {
        let stacks = self.stacks();
        let of = || stacks.iter().flatten().filter(|s| &*s.item == item);
        if of().any(|s| !plain(s)) {
            return 0;
        }
        of().map(|s| s.count).sum()
    }
    fn take(&mut self, item: &str, count: u32) -> u32 {
        if !smp_market::valid_id(item) || count == 0 {
            return 0;
        }
        let before = self.total(item);
        self.game.native_run_command(&format!("clear {} {item} {count}", self.name));
        before.saturating_sub(self.total(item))
    }
    fn room(&mut self, item: &str) -> u32 {
        self.stacks()
            .iter()
            .map(|s| match s {
                None => smp_shop::STACK,
                Some(s) if &*s.item == item && plain(s) => smp_shop::STACK.saturating_sub(s.count),
                Some(_) => 0,
            })
            .sum()
    }
    /// What doesn't fit (`room` counts 64 a stack) falls at the player's
    /// feet for them, as `give` does: given all the same.
    fn give(&mut self, item: &str, count: u32) -> u32 {
        if !smp_market::valid_id(item) || count == 0 {
            return 0;
        }
        let line = format!("give {} {item} {count}", self.name);
        if self.game.native_run_command(&line) { count } else { 0 }
    }
}

impl smp_auction::Hand for MenuBag<'_, '_> {
    fn held_whole(&self) -> Option<Result<smp_auction::Whole, String>> {
        None
    }
    fn take_held(&mut self, _held: &smp_auction::Whole, _count: u32) -> Option<smp_auction::Whole> {
        None
    }
    fn fits(&mut self, w: &smp_auction::Whole) -> bool {
        let Some(stack) = decode_stack(&w.data) else {
            return false;
        };
        let max = self.game.native_max_stack_size(&stack.item).unwrap_or(64).max(1);
        fits_in(&self.stacks(), &stack, max)
    }
    fn give_whole(&mut self, w: &smp_auction::Whole) -> Option<smp_auction::Whole> {
        give_whole_to(self.game, self.uuid, w)
    }
}

/// The shop menu's cells as items.
fn shop_items(smp: &Smp, uuid: u128) -> Vec<Option<ItemStack>> {
    let economy = smp.economy.as_ref().ok();
    let money = |n: i64| economy.map_or_else(|| n.to_string(), |e| e.money(n));
    let config = &smp.market.config;
    smp.shop
        .layout(&smp.shop_config, config, uuid)
        .into_iter()
        .map(|cell| {
            use smp_shop::Cell as C;
            Some(match cell {
                C::Empty => return None,
                C::Filler => button("minecraft:gray_stained_glass_pane", " ", "gray"),
                C::Category(icon, name, n) => button(&icon, &format!("{name} ({n} items)"), "yellow"),
                C::Item(id, price) => button(&id, &smp_shop::item_label(&id, price, &money), "white"),
                C::Back => button("minecraft:arrow", "Back", "white"),
                C::Prev => button("minecraft:arrow", "Previous page", "white"),
                C::Next => button("minecraft:arrow", "Next page", "white"),
                C::Balance => {
                    let b = economy.map_or(0, |e| e.balance(uuid));
                    button("minecraft:gold_ingot", &format!("Balance: {}", money(b)), "gold")
                }
                C::SellAll => button("minecraft:hopper", "Sell everything the market buys", "green"),
                C::Help => button("minecraft:book", smp_shop::HELP_TEXT, "white"),
                C::Close => button("minecraft:barrier", "Close", "red"),
            })
        })
        .collect()
}

/// `/shop`: the menu opens on its first page.
fn open_shop(game: &mut Game<'_>, smp: &mut Smp, me: &Player) -> Vec<Out> {
    if !smp.market.config.enabled || !smp.shop_config.enabled {
        return vec![Out::Error("The shop is closed.".into())];
    }
    if smp.trades.trade_of(me.uuid).is_some() {
        return vec![Out::Error("Finish or cancel your trade first.".into())];
    }
    smp.shop.views.insert(me.uuid, (None, smp_shop::Page::Main));
    let items = shop_items(smp, me.uuid);
    let title = Text::literal(smp.shop_config.title.clone());
    let window = game.native_open_menu(me.uuid, smp_shop::ROWS, title, items);
    match window {
        Some(w) => {
            smp.shop.views.insert(me.uuid, (Some(w), smp_shop::Page::Main));
            Vec::new()
        }
        None => {
            smp.shop.views.remove(&me.uuid);
            vec![Out::Error("The shop couldn't open.".into())]
        }
    }
}

/// A click in a player's shop menu: a page, a purchase or a sale, then the
/// menu shows the new balance and limits.
fn shop_clicked(game: &mut Game<'_>, smp: &mut Smp, uuid: u128, slot: i32, shift: bool) {
    use smp_shop::Click;
    let Some(p) = game.native_server_players().into_iter().find(|p| p.uuid == uuid) else {
        return;
    };
    let me = Player {
        uuid,
        name: p.name.clone(),
        level: p.permission,
    };
    // A click on a stack of the player's own sells it; one on the menu is
    // the shop's.
    let (sale, click) = match native_menu_inventory_slot(smp_shop::SIZE, slot) {
        Some(i) => match game.native_player_stacks(uuid).and_then(|s| s.get(i).cloned().flatten()) {
            Some(s) if !plain(&s) => {
                let why = "The market only takes plain items (not worn, named or enchanted).";
                tell(game, uuid, vec![Out::Error(why.into())]);
                return;
            }
            Some(s) => (Some((s.item.to_string(), s.count)), Click::Nothing),
            None => return,
        },
        None => match usize::try_from(slot) {
            Ok(s) => (None, smp.shop.click(&smp.shop_config, &smp.market.config, uuid, s, shift)),
            Err(_) => return,
        },
    };
    match (&sale, &click) {
        (None, Click::Nothing) => return,
        (_, Click::Close) => {
            smp.shop.views.remove(&uuid);
            game.native_close_menu(uuid);
            return;
        }
        _ => {}
    }
    let mut out = Vec::new();
    let moves = sale.is_some() || matches!(click, Click::Buy(..) | Click::SellAll);
    if moves && !matches!(p.game_mode, 0 | 2) {
        out.push(Out::Error("The market trades only with players in survival or adventure mode.".into()));
    } else if moves {
        let Ok(economy) = &mut smp.economy else {
            let why = "The economy is off: its file can't be read (see the server log).";
            tell(game, uuid, vec![Out::Error(why.into())]);
            return;
        };
        economy.now = chrono::Utc::now().timestamp_millis();
        economy.seen(uuid, &me.name);
        let mut bag = MenuBag {
            game: &mut *game,
            uuid,
            name: me.name.clone(),
        };
        out = match (sale, click) {
            (Some((id, n)), _) => smp.market.sell(economy, &me, &mut bag, &[&id, &n.to_string()]),
            (None, Click::Buy(id, n)) => smp.market.buy(economy, &me, &mut bag, &[&id, &n.to_string()]),
            _ => smp.market.sell(economy, &me, &mut bag, &["all"]),
        };
        // The menu shows the balance.
        out.retain(|o| !matches!(o, Out::Reply(s) if s.starts_with("Balance: ")));
    }
    let items = shop_items(smp, uuid);
    game.native_set_menu_items(uuid, items);
    tell(game, uuid, out);
}

/// The first map id of the market's charts: two per item (24 h, 7 days),
/// far above any map a world makes.
const CHART_MAPS: i32 = 1_900_000_000;

/// Files the SMP writes from threads (the prices, the web page): one at a
/// time, as each goes through the same temporary name.
static WRITES: Mutex<()> = Mutex::new(());

/// `value` written as compact JSON by a thread, after the one before.
fn write_later<T: serde::Serialize + Send + 'static>(path: PathBuf, value: T) {
    std::thread::spawn(move || {
        let _one = WRITES.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = smp_store::write_json_compact(&path, &value) {
            log::warn!("[smp] {e}");
        }
    });
}

/// Lines under an item's name in a menu (white unless given, not italic).
fn with_lore(mut stack: ItemStack, lines: &[(String, &str)]) -> ItemStack {
    let lines = lines
        .iter()
        .map(|(text, color)| component_nbt(&serde_json::json!({ "text": text, "italic": false, "color": color })))
        .collect();
    stack.data_mut().set_other("minecraft:lore", Some(horizon_world::nbt::Tag::List(lines)));
    stack
}

/// A change in percent as a coloured line part: `▲ 2.4%` green, `▼` red.
fn change_line(label: &str, change: Option<f64>) -> (String, &'static str) {
    match change {
        Some(c) if c >= 0.05 => (format!("{label} {}", smp_market_prices::arrow(c)), "green"),
        Some(c) if c <= -0.05 => (format!("{label} {}", smp_market_prices::arrow(c)), "red"),
        Some(_) => (format!("{label} unchanged"), "gray"),
        None => (format!("{label} –"), "gray"),
    }
}

/// An item's prices and moves, as lore lines.
fn price_lines(smp: &Smp, id: &str, now: i64) -> Vec<(String, &'static str)> {
    let each = smp.market.each(id);
    let each = smp.economy.as_ref().map_or_else(|_| format!("{each:.2}"), |e| smp_market::each_money(e, each));
    let p = smp.market.base(id);
    let mut lines = Vec::new();
    match (p.buy > 0, p.sell > 0) {
        (true, true) => lines.push((format!("Price: {each} each, to buy or sell"), "gold")),
        (true, false) => lines.push((format!("Price: {each} each (the market doesn't buy it)"), "gold")),
        (false, true) => lines.push((format!("Price: {each} each (the market doesn't sell it)"), "gold")),
        (false, false) => {}
    }
    lines.push(change_line("1 h", smp.market.change(id, now, 60)));
    lines.push(change_line("24 h", smp.market.change(id, now, 24 * 60)));
    lines.push((format!("Traded in 24 h: {}", smp.market.volume(id, now, 24)), "aqua"));
    lines
}

/// The `/market` menu's cells as items.
fn market_items(smp: &Smp, uuid: u128, now: i64) -> Vec<Option<ItemStack>> {
    use smp_market_menu::{Cell as C, Chart};
    let economy = smp.economy.as_ref().ok();
    let money = |n: i64| economy.map_or_else(|| n.to_string(), |e| e.money(n));
    let name = |id: &str| smp_market::title_name(id);
    smp.market_menus
        .layout(&smp.shop_config, &smp.market.config, uuid)
        .into_iter()
        .map(|cell| {
            Some(match cell {
                C::Empty => return None,
                C::Filler => button("minecraft:gray_stained_glass_pane", " ", "gray"),
                C::Category(icon, title, n) => {
                    with_lore(button(&icon, &title, "yellow"), &[(format!("{n} items"), "gray")])
                }
                C::Item(id) => {
                    let mut lines = price_lines(smp, &id, now);
                    lines.push(("Click for its page".into(), "gray"));
                    with_lore(button(&id, &name(&id), "white"), &lines)
                }
                C::Detail(id) => {
                    let mut lines = price_lines(smp, &id, now);
                    let base = smp.market.base(&id);
                    lines.push((format!("Base price: {}", money(base.buy.max(base.sell))), "gray"));
                    with_lore(button(&id, &name(&id), "white"), &lines)
                }
                C::Buy(id, n) => {
                    // What these n cost now, the price moving as they go.
                    let cost = smp.market.value(&id, n, true).0;
                    let mut b = button("minecraft:lime_concrete", &format!("Buy {n} for {}", money(cost)), "green");
                    b.count = n;
                    b
                }
                C::Sell(id, n) => {
                    let gets = smp.market.value(&id, n, false).0;
                    let mut b = button("minecraft:red_concrete", &format!("Sell {n} for {}", money(gets)), "red");
                    b.count = n;
                    b
                }
                C::SellAll(id) => button("minecraft:hopper", &format!("Sell all your {}", name(&id)), "red"),
                C::Chart(id, chart) => {
                    let span = if chart == Chart::Day { "24 hours" } else { "7 days" };
                    with_lore(
                        button("minecraft:map", &format!("Price chart, {span}"), "aqua"),
                        &[(format!("Gives a map of {}'s price", name(&id)), "gray"), ("It updates each minute while you carry it".into(), "gray")],
                    )
                }
                C::News => {
                    let lines = movers(smp, now);
                    let lines: Vec<(String, &str)> = if lines.is_empty() {
                        vec![("No move yet today".into(), "gray")]
                    } else {
                        lines
                    };
                    with_lore(button("minecraft:paper", "Movers over 24 hours", "yellow"), &lines)
                }
                C::Back => button("minecraft:arrow", "Back", "white"),
                C::Prev => button("minecraft:arrow", "Previous page", "white"),
                C::Next => button("minecraft:arrow", "Next page", "white"),
                C::Balance => {
                    let b = economy.map_or(0, |e| e.balance(uuid));
                    button("minecraft:gold_ingot", &format!("Balance: {}", money(b)), "gold")
                }
                C::Close => button("minecraft:barrier", "Close", "red"),
            })
        })
        .collect()
}

/// The three biggest risers and fallers over 24 h (at least 1%).
fn movers(smp: &Smp, now: i64) -> Vec<(String, &'static str)> {
    let mut moves: Vec<(String, f64)> = smp
        .market
        .config
        .items
        .keys()
        .filter_map(|id| Some((smp_market::title_name(id), smp.market.change(id, now, 24 * 60)?)))
        .filter(|(_, c)| c.abs() >= 1.0)
        .collect();
    moves.sort_by(|a, b| b.1.total_cmp(&a.1));
    let up = moves.iter().filter(|m| m.1 > 0.0).take(3);
    let down = moves.iter().rev().filter(|m| m.1 < 0.0).take(3);
    up.map(|(n, c)| (format!("{n} {}", smp_market_prices::arrow(*c)), "green"))
        .chain(down.map(|(n, c)| (format!("{n} {}", smp_market_prices::arrow(*c)), "red")))
        .collect()
}

/// `/market [item]`: the menu, on its first page or the item's.
fn open_market(game: &mut Game<'_>, smp: &mut Smp, me: &Player, line: &str, now: i64) -> Vec<Out> {
    if !smp.market.config.enabled {
        return vec![Out::Error("The market is closed.".into())];
    }
    if smp.trades.trade_of(me.uuid).is_some() {
        return vec![Out::Error("Finish or cancel your trade first.".into())];
    }
    let page = match line.split_whitespace().nth(1) {
        None => smp_market_menu::Page::Main,
        Some(item) => {
            let id = smp_market::item_id(item);
            match smp_market_menu::Menus::page_of(&smp.shop_config, &smp.market.config, &id) {
                Some(p) => p,
                None => return vec![Out::Error(format!("The market doesn't trade {}.", smp_market::item_name(&id)))],
            }
        }
    };
    smp.market_menus.views.insert(me.uuid, (None, page.clone()));
    let items = market_items(smp, me.uuid, now);
    let window = game.native_open_menu(me.uuid, smp_market_menu::ROWS, Text::literal("Market"), items);
    match window {
        Some(w) => {
            smp.market_menus.views.insert(me.uuid, (Some(w), page));
            Vec::new()
        }
        None => {
            smp.market_menus.views.remove(&me.uuid);
            vec![Out::Error("The market couldn't open.".into())]
        }
    }
}

/// A click in a player's `/market` menu: a page, a trade or a chart.
fn market_clicked(game: &mut Game<'_>, smp: &mut Smp, uuid: u128, slot: i32) {
    use smp_market_menu::Click;
    let Some(p) = game.native_server_players().into_iter().find(|p| p.uuid == uuid) else {
        return;
    };
    let me = Player {
        uuid,
        name: p.name.clone(),
        level: p.permission,
    };
    let Ok(slot) = usize::try_from(slot) else {
        return;
    };
    let now = chrono::Utc::now().timestamp_millis();
    let click = smp.market_menus.click(&smp.shop_config, &smp.market.config, uuid, slot);
    let mut out = Vec::new();
    match click {
        Click::Nothing => return,
        Click::Close => {
            smp.market_menus.views.remove(&uuid);
            game.native_close_menu(uuid);
            return;
        }
        Click::Show => {}
        Click::Chart(id, chart) => out.extend(give_chart(game, smp, uuid, &id, chart, now)),
        Click::Buy(..) | Click::Sell(..) | Click::SellAll(..) if !matches!(p.game_mode, 0 | 2) => {
            out.push(Out::Error("The market trades only with players in survival or adventure mode.".into()));
        }
        trade => {
            let Ok(economy) = &mut smp.economy else {
                let why = "The economy is off: its file can't be read (see the server log).";
                tell(game, uuid, vec![Out::Error(why.into())]);
                return;
            };
            economy.now = now;
            economy.seen(uuid, &me.name);
            let mut bag = MenuBag {
                game: &mut *game,
                uuid,
                name: me.name.clone(),
            };
            out = match trade {
                Click::Buy(id, n) => smp.market.buy(economy, &me, &mut bag, &[&id, &n.to_string()]),
                Click::Sell(id, n) => smp.market.sell(economy, &me, &mut bag, &[&id, &n.to_string()]),
                Click::SellAll(id) => smp.market.sell(economy, &me, &mut bag, &[&id]),
                _ => Vec::new(),
            };
            out.retain(|o| !matches!(o, Out::Reply(s) if s.starts_with("Balance: ")));
        }
    }
    let items = market_items(smp, uuid, now);
    game.native_set_menu_items(uuid, items);
    tell(game, uuid, out);
}

/// Which chart a map id is: the item and the span.
fn chart_of(smp: &Smp, map_id: i32) -> Option<(String, smp_market_menu::Chart)> {
    let n = usize::try_from(map_id.checked_sub(CHART_MAPS)?).ok()?;
    let id = smp.market.config.items.keys().nth(n / 2)?;
    let chart = if n % 2 == 0 { smp_market_menu::Chart::Day } else { smp_market_menu::Chart::Week };
    Some((id.clone(), chart))
}

/// The picture of a chart now.
fn chart_picture(smp: &Smp, id: &str, chart: smp_market_menu::Chart, now: i64) -> Vec<u8> {
    use smp_market_prices::{HOUR_MS, Span};
    let (span, label, since) = match chart {
        smp_market_menu::Chart::Day => (Span::Minute, "24H", now - 24 * HOUR_MS),
        smp_market_menu::Chart::Week => (Span::Hour, "7D", now - 7 * 24 * HOUR_MS),
    };
    let mut prices: Vec<f64> =
        smp.market.history(id, span).iter().filter(|p| p.t >= since).map(|p| p.price).collect();
    if let Some(q) = smp.market.spot(id) {
        prices.push(q.mid);
    }
    smp_market_chart::render(&smp_market::title_name(id), label, &prices)
}

/// A chart's map to the player, and its picture.
fn give_chart(
    game: &mut Game<'_>,
    smp: &mut Smp,
    uuid: u128,
    id: &str,
    chart: smp_market_menu::Chart,
    now: i64,
) -> Vec<Out> {
    let Some(index) = smp.market.config.items.keys().position(|k| k == id) else {
        return Vec::new();
    };
    let Some(map_id) = i32::try_from(index * 2 + usize::from(chart == smp_market_menu::Chart::Week))
        .ok()
        .and_then(|n| CHART_MAPS.checked_add(n))
    else {
        return Vec::new();
    };
    let span = if chart == smp_market_menu::Chart::Day { "24 hours" } else { "7 days" };
    let name = smp_market::title_name(id);
    let mut map = button("minecraft:filled_map", &format!("{name}: price over {span}"), "aqua");
    map.data_mut().map_id = Some(map_id);
    let map = with_lore(map, &[("Updates each minute while you carry it".into(), "gray")]);
    if game.native_give_stack(uuid, map).is_some() {
        return vec![Out::Error("No room in your inventory for the chart.".into())];
    }
    let picture = chart_picture(smp, id, chart, now);
    game.native_send_map(uuid, map_id, &picture);
    vec![Out::Reply(format!("Here is {name}'s chart. Hold it or put it in an item frame."))]
}

/// Once a minute when prices moved: open `/market` menus show the new
/// prices, carried charts are drawn again, and the web page is written.
fn market_minute(game: &mut Game<'_>, smp: &mut Smp, players: &[NativeServerPlayer], now: i64) {
    let open: Vec<u128> = smp.market_menus.views.iter().filter(|v| v.1.0.is_some()).map(|v| *v.0).collect();
    for uuid in open {
        let items = market_items(smp, uuid, now);
        game.native_set_menu_items(uuid, items);
    }
    // Charts carried (any slot): a scan of the players on, once a minute.
    {
        let mut pictures: std::collections::HashMap<i32, Vec<u8>> = std::collections::HashMap::new();
        for p in players {
            let Some(stacks) = game.native_player_stacks(p.uuid) else {
                continue;
            };
            let maps: Vec<i32> = stacks
                .iter()
                .flatten()
                .filter(|s| &*s.item == "minecraft:filled_map")
                .filter_map(|s| s.data().and_then(|d| d.map_id))
                .filter(|m| *m >= CHART_MAPS)
                .collect();
            for m in maps {
                let Some((id, chart)) = chart_of(smp, m) else {
                    continue;
                };
                let picture = pictures.entry(m).or_insert_with(|| chart_picture(smp, &id, chart, now));
                game.native_send_map(p.uuid, m, picture);
            }
        }
    }
    let currency = smp.economy.as_ref().map_or_else(|_| String::new(), |e| e.config.currency.clone());
    let categories: Vec<(String, Vec<String>)> = smp_market_menu::categories(&smp.shop_config, &smp.market.config)
        .into_iter()
        .map(|(_, name, items)| (name, items))
        .collect();
    let view = smp.market.web_view();
    let path = smp.dir.join("web").join("market.json");
    let web = smp.web.clone();
    std::thread::spawn(move || {
        let doc = view.doc(&categories, &currency, now);
        {
            let _one = WRITES.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = smp_store::write_json_compact(&path, &doc) {
                log::warn!("[smp] web/market.json: {e}");
            }
        }
        if let Some(dir) = path.parent() {
            push_pages(dir, &web);
        }
    });
}

/// A send of the web pages under way (one at a time).
static PUSHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// What was last sent of each page (a hash of its bytes).
static PUSHED: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());

/// Each web page that changed since it was last sent, POSTed gzipped to
/// `web.url/<name>` with the token (never logged). Off the tick thread.
fn push_pages(dir: &std::path::Path, web: &WebPush) {
    use std::hash::{Hash, Hasher};
    use std::io::Write;
    use std::sync::atomic::Ordering::SeqCst;
    if web.token.trim().is_empty() || web.url.trim().is_empty() || PUSHING.swap(true, SeqCst) {
        return;
    }
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(20)).build();
    for name in ["market.json", "economy.json"] {
        let bytes = {
            let _one = WRITES.lock().unwrap_or_else(|e| e.into_inner());
            match std::fs::read(dir.join(name)) {
                Ok(b) => b,
                Err(_) => continue,
            }
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut h);
        let hash = h.finish();
        let sent = |pushed: &Vec<(&'static str, u64)>| pushed.iter().any(|p| *p == (name, hash));
        if sent(&PUSHED.lock().unwrap_or_else(|e| e.into_inner())) {
            continue;
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let Ok(body) = gz.write_all(&bytes).and_then(|()| gz.finish()) else {
            continue;
        };
        let url = format!("{}/{name}", web.url.trim().trim_end_matches('/'));
        let call = agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", web.token.trim()))
            .set("Content-Type", "application/json")
            .set("Content-Encoding", "gzip")
            .send_bytes(&body);
        match call {
            Ok(_) => {
                let mut pushed = PUSHED.lock().unwrap_or_else(|e| e.into_inner());
                pushed.retain(|p| p.0 != name);
                pushed.push((name, hash));
            }
            // The status and the address only, never the token.
            Err(ureq::Error::Status(code, _)) => log::warn!("[smp] sending {name} to {url}: HTTP {code}"),
            Err(e) => log::warn!("[smp] sending {name} to {url}: {}", e.kind()),
        }
    }
    PUSHING.store(false, SeqCst);
}

/// `/ah ...`: the auction house's commands, or its menu.
fn ah_command(game: &mut Game<'_>, smp: &mut Smp, me: &Player, trades: bool, now: i64, line: &str) -> Vec<Out> {
    let (Ok(e), Some(a)) = (&mut smp.economy, &mut smp.auction) else {
        return vec![Out::Error("The economy is off: its file can't be read (see the server log).".into())];
    };
    e.seen(me.uuid, &me.name);
    let played = smp.places.as_ref().map_or(0, |p| p.played_seconds(me.uuid));
    let reply = {
        let mut bag = GameBag(game);
        a.command(e, me, trades, &mut bag, played, now, line)
    };
    match reply {
        Some(smp_auction::Reply::Out(out)) => out,
        Some(smp_auction::Reply::Open(page)) => open_ah(game, smp, me.uuid, page),
        None => Vec::new(),
    }
}

/// The auction house's menu opens on `page`.
fn open_ah(game: &mut Game<'_>, smp: &mut Smp, uuid: u128, page: smp_auction::Page) -> Vec<Out> {
    if smp.trades.trade_of(uuid).is_some() {
        return vec![Out::Error("Finish or cancel your trade first.".into())];
    }
    let Some(a) = &mut smp.auction else {
        return Vec::new();
    };
    a.views.insert(uuid, (None, page.clone()));
    let items = ah_items(smp, uuid);
    let window = game.native_open_menu(uuid, smp_auction::ROWS, Text::literal("Auction house"), items);
    let a = smp.auction.as_mut().expect("checked");
    match window {
        Some(w) => {
            a.views.insert(uuid, (Some(w), page));
            Vec::new()
        }
        None => {
            a.views.remove(&uuid);
            vec![Out::Error("The auction house couldn't open.".into())]
        }
    }
}

/// An item stack of `count` (1 to 64) named `name`.
fn named_stack(item: &str, count: u32, name: &str, color: &str) -> ItemStack {
    let mut stack = button(item, name, color);
    stack.count = count.clamp(1, 64);
    stack
}

/// The auction house menu's cells as items.
fn ah_items(smp: &Smp, uuid: u128) -> Vec<Option<ItemStack>> {
    let Some(a) = &smp.auction else {
        return Vec::new();
    };
    let economy = smp.economy.as_ref().ok();
    let money = |n: i64| economy.map_or_else(|| n.to_string(), |e| e.money(n));
    let now = chrono::Utc::now().timestamp_millis();
    let cats = &smp.shop_config.categories;
    a.layout(uuid, cats, now)
        .into_iter()
        .map(|cell| {
            use smp_auction::Cell as C;
            Some(match cell {
                C::Empty => return None,
                C::Filler => button("minecraft:gray_stained_glass_pane", " ", "gray"),
                C::Category(icon, name, n) => button(&icon, &format!("{name} ({n} listings)"), "yellow"),
                C::Listing(id) => {
                    let l = a.listing(id)?;
                    let color = if l.auction { "gold" } else { "white" };
                    let label = a.label(l, now, &money);
                    match l.stack.as_deref().and_then(decode_stack) {
                        // The stack itself (its name, enchantments, contents
                        // in the tooltip), the listing's facts under it.
                        Some(stack) => {
                            let lines: Vec<(String, &str)> = label.split(" · ").map(|p| (p.to_owned(), color)).collect();
                            with_lore(stack, &lines)
                        }
                        None => named_stack(&l.item, l.count, &label, color),
                    }
                }
                C::Parcel(i) => {
                    let p = a.parcel(uuid, i)?;
                    let name = format!("Claim {} {} ({})", p.count, smp_shop::title(&p.item), p.why);
                    match p.stack.as_deref().and_then(decode_stack) {
                        Some(stack) => with_lore(stack, &[(name, "green")]),
                        None => named_stack(&p.item, p.count, &name, "green"),
                    }
                }
                C::Confirm(id) => {
                    let l = a.listing(id)?;
                    button("minecraft:lime_concrete", &a.confirm_label(l, &money), "green")
                }
                C::Cancel => button("minecraft:red_stained_glass_pane", "Back", "red"),
                C::Back => button("minecraft:arrow", "Back", "white"),
                C::Prev => button("minecraft:arrow", "Previous page", "white"),
                C::Next => button("minecraft:arrow", "Next page", "white"),
                C::Mine(n) => button("minecraft:chest", &format!("Your listings ({n}): click one to take it back"), "aqua"),
                C::Mail(n) => button("minecraft:barrel", &format!("Your mail ({n} parcels)"), "aqua"),
                C::ClaimAll => button("minecraft:hopper", "Claim everything", "green"),
                C::Balance => {
                    let b = economy.map_or(0, |e| e.balance(uuid));
                    button("minecraft:gold_ingot", &format!("Balance: {}", money(b)), "gold")
                }
                C::Help => button("minecraft:book", smp_auction::HELP_TEXT, "white"),
                C::Close => button("minecraft:barrier", "Close", "red"),
            })
        })
        .collect()
}

/// A click in a player's auction house menu: a page, or a purchase, a bid,
/// a withdrawal, a claim; then the menu is drawn again.
fn ah_clicked(game: &mut Game<'_>, smp: &mut Smp, uuid: u128, slot: i32) {
    use smp_auction::Click;
    let Some(p) = game.native_server_players().into_iter().find(|p| p.uuid == uuid) else {
        return;
    };
    // Clicks on the player's own inventory do nothing here.
    let Ok(slot) = usize::try_from(slot) else {
        return;
    };
    if slot >= smp_auction::SIZE {
        return;
    }
    let me = Player {
        uuid,
        name: p.name.clone(),
        level: p.permission,
    };
    let now = chrono::Utc::now().timestamp_millis();
    let cats = smp.shop_config.categories.clone();
    let (Ok(e), Some(a)) = (&mut smp.economy, &mut smp.auction) else {
        return;
    };
    let click = a.click(uuid, &cats, slot, now);
    if matches!(click, Click::Buy(_) | Click::Bid(_)) && !matches!(p.game_mode, 0 | 2) {
        let why = "The auction house trades only with players in survival or adventure mode.";
        tell(game, uuid, vec![Out::Error(why.into())]);
        return;
    }
    e.now = now;
    e.seen(uuid, &me.name);
    let mut bag = MenuBag {
        game: &mut *game,
        uuid,
        name: me.name.clone(),
    };
    let out = match click {
        Click::Nothing => return,
        Click::Close => {
            a.views.remove(&uuid);
            game.native_close_menu(uuid);
            return;
        }
        Click::Redraw => Vec::new(),
        Click::Buy(id) => a.buy(e, &me, &mut bag, id, now),
        Click::Bid(id) => a.bid(e, &me, id, None, now),
        Click::Withdraw(id) => {
            let mut out = a.withdraw(&me, id);
            if !matches!(out.first(), Some(Out::Error(_))) {
                out.extend(a.claim(uuid, &mut bag, None));
            }
            out
        }
        Click::Claim(i) => a.claim(uuid, &mut bag, Some(i)),
        Click::ClaimAll => a.claim(uuid, &mut bag, None),
    };
    // After a purchase or a bid, back to the list it came from.
    if !out.is_empty()
        && let Some(v) = a.views.get_mut(&uuid)
        && let smp_auction::Page::Confirm(_, back) = &v.1
    {
        v.1 = (**back).clone();
    }
    let items = ah_items(smp, uuid);
    game.native_set_menu_items(uuid, items);
    tell(game, uuid, out);
}

/// What a menu click answers, to the player who clicked (no command's
/// feedback there).
fn tell(game: &mut Game<'_>, uuid: u128, out: Vec<Out>) {
    for o in out {
        match o {
            Out::Reply(s) => {
                game.native_tell(uuid, Text::literal(s).with_color(text::GREEN));
            }
            Out::Error(s) => {
                game.native_tell(uuid, Text::literal(s).with_color(text::RED));
            }
            o => show(game, vec![o]),
        }
    }
}

fn show(game: &mut Game<'_>, out: Vec<Out>) {
    for o in out {
        match o {
            Out::Reply(s) => game.native_feedback(Text::literal(s)),
            Out::Error(s) => game.native_feedback(Text::literal(s).with_color(text::RED)),
            Out::Tell(uuid, s) => {
                game.native_tell(uuid, Text::literal(s).with_color(text::GOLD));
            }
            Out::All(s) => game.native_broadcast(Text::literal(s).with_color(text::GOLD)),
        }
    }
}

/// The end of each server tick: accounts for the players on (once a second)
/// and the files that changed.
pub(in crate::game) fn tick(game: &mut Game<'_>) {
    use std::sync::atomic::Ordering::Relaxed;
    let n = TICKS.fetch_add(1, Relaxed);
    // An idle tick costs a counter: the rest runs once a second, or each
    // tick while something waits on it.
    if n % 20 != 0 && !BUSY.load(Relaxed) {
        return;
    }
    let Some(dir) = game.native_world_dir() else {
        return;
    };
    let mut guard = state(dir.join(smp_store::DIR));
    let smp = guard.as_mut().expect("opened");
    let warming = smp.places.as_ref().is_ok_and(|p| !p.pending.is_empty());
    let players = if n % 20 == 0 || warming {
        game.native_server_players()
    } else {
        Vec::new()
    };
    let now = chrono::Utc::now().timestamp_millis();
    let mut out = Vec::new();
    // Where each player stands, for where they leave from (once a second).
    if n % 20 == 0 {
        for b in bodies(&players) {
            smp.standing.insert(b.uuid, b.at);
        }
    }
    note_leavers(smp);
    // `/vanish`, once a second: the game told who is hidden (after a
    // restart), and the subtitle sent to each vanished player on.
    let vanished = smp.vanish.as_ref().map(|v| v.quiet()).unwrap_or_default();
    let (mut subtitles, mut unvanished) = (Vec::new(), Vec::new());
    if n % 20 == 0
        && let Ok(v) = &mut smp.vanish
    {
        let _span = horizon_client::spans::guard("smp_vanish");
        for (uuid, hidden) in v.changes() {
            game.native_hide_player(uuid, hidden.then_some(smp_vanish::LEVEL));
        }
        for p in &players {
            if vanished.contains(&p.uuid) {
                subtitles.push((p.uuid, p.name.clone()));
            } else if smp.vanish_titled.contains(&p.uuid) {
                unvanished.push((p.uuid, p.name.clone()));
            }
        }
        smp.vanish_titled = subtitles.iter().map(|s| s.0).collect();
        if let Ok(p) = &mut smp.presence {
            p.quiet = vanished.clone();
        }
    }
    // Teleports: each tick while one warms up; requests expire once a second.
    // With places off only the playtime counts.
    let places_on = smp.switches.on(Part::Places);
    if let Ok(p) = &mut smp.places
        && (n % 20 == 0 || warming)
    {
        let _span = horizon_client::spans::guard("smp_places");
        if n % 20 == 0 {
            let on: Vec<u128> = players.iter().map(|p| p.uuid).collect();
            p.played(&on, now);
        }
        let (o, go) = if places_on { p.tick(&bodies(&players), now) } else { Default::default() };
        out.extend(o);
        for t in go {
            // `.3`: a whole number would be a block's centre to `tp`.
            let line = format!(
                "execute in {} run tp {} {:.3} {:.3} {:.3}",
                t.to.dimension, t.name, t.to.x, t.to.y, t.to.z
            );
            if !game.native_run_command(&line) {
                log::warn!("[smp] a teleport failed: {line}");
                p.call_off(&t);
                out.push(Out::Tell(t.uuid, "The teleport failed.".into()));
                continue;
            }
            // `/spawn`: on to a safe column within the radius, as vanilla's
            // respawn does. If none is found the player stays at the spawn.
            if let Some(spread) = t.spread_line()
                && !game.native_run_command(&spread)
            {
                log::warn!("[smp] no safe spot near the spawn: {spread}");
            }
            if t.what == "a random spot" {
                let mut cost = RTP_COST.lock().unwrap_or_else(|e| e.into_inner());
                let (_, n, total, max) = cost.iter().find(|c| c.0 == t.uuid).copied().unwrap_or_default();
                cost.retain(|c| c.0 != t.uuid);
                log::info!(
                    "[smp] /rtp: {} to {:.1} {:.1} {:.1} ({n} chunk reads on the tick: {total} µs, longest {max} µs)",
                    t.name, t.to.x, t.to.y, t.to.z
                );
            }
            out.push(Out::Tell(t.uuid, t.done.clone()));
        }
        // `/rtp`'s chunks: held loaded while it looks (a ticket, which never
        // waits for the chunk), let go after.
        let legacy = p.force(now, &mut |(x, z), hold| {
            game.native_force_chunk_in(RTP_TICKET, smp_places::OVERWORLD, [x, z], hold)
        });
        for line in legacy {
            game.native_run_command(&line);
        }
        RTP.store(p.rtp_searching(), Relaxed);
    }
    if let Ok(e) = &mut smp.economy
        && n % 20 == 0
    {
        let _span = horizon_client::spans::guard("smp_economy");
        for p in &players {
            e.seen(p.uuid, &p.name);
        }
        // Once a second: coinflips nobody took, and those of players gone.
        out.extend(smp.games.expire(e, chrono::Utc::now().timestamp_millis()));
        let gone: Vec<u128> = smp
            .games
            .offers
            .iter()
            .map(|o| o.from)
            .filter(|u| !players.iter().any(|p| p.uuid == *u))
            .collect();
        for uuid in gone {
            smp.games.left(e, uuid);
        }
        // A part turned off waits: its draws, endings, markings and payments
        // happen once it is on again (coinflips not taken still go back).
        let on = |p| smp.switches.on(p);
        if on(Part::Gambling) {
            out.extend(smp.lottery.tick(&smp.games.config, e, now, &mut below));
        }
        // The auction house: notes for who came on, listings that ended.
        if let Some(a) = &mut smp.auction
            && on(Part::Auction)
        {
            let on: Vec<u128> = players.iter().map(|p| p.uuid).collect();
            out.extend(a.tick(e, &on, now));
        }
        // The daily report and, once a minute, web/economy.json (written off
        // the tick thread).
        if let Some(r) = &mut smp.report
            && r.due(now)
        {
            let _span = horizon_client::spans::guard("smp_report");
            let (o, web) = r.tick(e, now);
            out.extend(o);
            if web {
                let doc = r.web(e, now);
                let path = smp.dir.join("web").join("economy.json");
                std::thread::spawn(move || {
                    let made = path.parent().map_or(Ok(()), std::fs::create_dir_all);
                    if let Err(err) = made.map_err(|e| e.to_string()).and_then(|()| smp_store::write_json(&path, &doc)) {
                        log::warn!("[smp] web/economy.json: {err}");
                    }
                });
            }
        }
    }
    // The market's minute: prices move, open menus and carried charts
    // follow, the files are written by threads; once a day, the movers.
    if n % 20 == 0 {
        let _span = horizon_client::spans::guard("smp_market");
        for w in smp.market.warnings.drain(..) {
            log::warn!("[smp] market: {w}");
        }
        let moved = smp.market.tick(now);
        if moved || now - smp.market_web >= 5 * 60_000 {
            let _span = horizon_client::spans::guard("smp_market_minute");
            smp.market_web = now;
            market_minute(game, smp, &players, now);
        }
        if let Some((path, book)) = smp.market.book_to_write(now, false) {
            write_later(path, book);
        }
        if let Some(lines) = smp.market.summary_due(now) {
            for l in lines {
                log::info!("[smp] market: {l}");
                out.push(Out::All(l));
            }
        }
    }
    // Each trade's offers against the inventories now: a player gone or
    // dead ends it; a stack that changed leaves its offer.
    let sides: Vec<(u128, String)> = smp
        .trades
        .open
        .iter()
        .flat_map(|t| t.sides.iter().map(|s| (s.uuid, s.name.clone())))
        .collect();
    for (uuid, name) in sides {
        match game.native_player_stacks(uuid) {
            None => {
                let (o, acts) = smp.trades.end(uuid, &format!("{name} is gone"));
                out.extend(o);
                act(game, smp, acts);
            }
            Some(stacks) if smp.trades.refresh(uuid, &stacks) => {
                let other = smp.trades.trade_of(uuid).and_then(|t| {
                    t.sides.iter().find(|s| s.uuid != uuid).map(|s| s.uuid)
                });
                act(game, smp, vec![Act::Show(uuid)]);
                if let Some(o) = other {
                    act(game, smp, vec![Act::Show(o)]);
                    out.push(Out::Tell(o, format!("{name}'s offer changed.")));
                }
                out.push(Out::Tell(uuid, "A stack you offered changed: it left your offer.".into()));
            }
            Some(_) => {}
        }
    }
    // Welcomes, daily rewards and AFK, once a second.
    let mut sleeps = std::mem::take(&mut *PENDING_SLEEP.lock().unwrap_or_else(|e| e.into_inner()));
    let economy_on = smp.switches.on(Part::Economy);
    if n % 20 == 0
        && smp.switches.on(Part::Presence)
        && let Ok(pr) = &mut smp.presence
    {
        let _span = horizon_client::spans::guard("smp_presence");
        let seen: Vec<smp_presence::Seen> = players
            .iter()
            .map(|p| smp_presence::Seen {
                uuid: p.uuid,
                name: p.name.clone(),
                pose: [p.position[0], p.position[1], p.position[2], f64::from(p.yaw), f64::from(p.pitch)],
            })
            .collect();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let rules = smp.places.as_ref().map(|p| p.config.rules.clone()).unwrap_or_default();
        let (o, acts) = pr.tick(&seen, &today, now, &rules);
        out.extend(o);
        let reward = pr.config.daily_reward;
        for a in acts {
            match a {
                smp_presence::Act::Reward(uuid) => {
                    if let Ok(e) = &mut smp.economy
                        && economy_on
                    {
                        match e.adjust("daily_reward", uuid, reward, "") {
                            Ok(_) => out.push(Out::Tell(uuid, format!("Daily reward: {}.", e.money(reward)))),
                            Err(err) => log::warn!("[smp] daily reward: {err}"),
                        }
                    }
                }
                a => sleeps.push(a),
            }
        }
    }
    // `/report`: operators who came on hear of the last day's reports.
    if n % 20 == 0 {
        let on: Vec<(u128, u8)> = players.iter().map(|p| (p.uuid, p.permission)).collect();
        out.extend(smp.reports.staff_joined(&on, now));
    }
    // `/claim show`: the border once a second, to that player only.
    let mut particles = Vec::new();
    if n % 20 == 0 {
        smp.shows.retain(|_, s| s.3 > now);
        for (uuid, (dimension, cx, cz, _)) in &smp.shows {
            let Some(p) = players.iter().find(|p| p.uuid == *uuid && p.dimension == *dimension) else {
                continue;
            };
            let relation = smp.claims.as_ref().ok().and_then(|c| c.relation(dimension, cx * 16, cz * 16, *uuid));
            let color = match relation {
                Some(true) => "0,1,0",
                Some(false) => "1,0,0",
                None => "1,1,1",
            };
            for [x, y, z] in smp_claims::border(*cx, *cz, p.position[1] + 1.0, 2) {
                particles.push(format!(
                    "execute in {dimension} run particle minecraft:dust{{color:[{color}],scale:1.5}} {x:.1} {y:.1} {z:.1} 0 0 0 0 1 force {}",
                    p.name
                ));
            }
        }
    }
    // The sidebars and the Tab list, once a second, sent only where they
    // changed.
    let mut sidebars = Vec::new();
    let mut tab = None;
    let (nicks, places) = (&smp.nicks, &smp.places);
    if n % 20 == 0
        && smp.switches.on(Part::Board)
        && let Ok(b) = &mut smp.board
    {
        let _span = horizon_client::spans::guard("smp_board");
        let on: Vec<u128> = players.iter().map(|p| p.uuid).collect();
        if let Ok(e) = &smp.economy
            && b.due(now, e.revision())
        {
            b.rebuild(e.accounts(), now, e.revision());
        }
        // Asked only for the players whose sidebar is looked at.
        let me = |uuid: u128| smp_board::Me {
            name: nicks
                .as_ref()
                .ok()
                .and_then(|n| n.nick_of(uuid))
                .map(|n| format!("~{n}"))
                .or_else(|| players.iter().find(|p| p.uuid == uuid).map(|p| p.name.clone()))
                .unwrap_or_default(),
            played_seconds: places.as_ref().map_or(0, |p| p.played_seconds(uuid)),
        };
        b.quiet = vanished.clone();
        sidebars = b.updates(now, &on, me);
        let to = b.tab_due(now, &on);
        if !to.is_empty() {
            let (rate, mspt) = game.native_tick_stats();
            let tps = f64::from(rate).min(1000.0 / mspt.max(0.001));
            // Vanished players aren't counted: the footer is everyone's.
            let shown = on.iter().filter(|u| !b.quiet.contains(u)).count();
            tab = Some((to, b.tab_lines(tps, mspt, shown)));
        }
    }
    // Nicknames: the changes asked for, then those of players who came on.
    if n % 20 == 0
        && smp.switches.on(Part::Nicks)
        && let Ok(nicks) = &mut smp.nicks
    {
        let _span = horizon_client::spans::guard("smp_nicks");
        let pending = std::mem::take(&mut nicks.pending);
        // The files behind the real names are read only when a change waits.
        let names = (!pending.is_empty()).then(|| ServerNames::read(&dir, smp.economy.as_ref().ok()));
        for change in pending {
            let names = names.as_ref().expect("read above");
            let real = names.name_of(change.uuid).unwrap_or_default();
            if let Some(nick) = &change.nick
                && game.native_text_allowed(nick) == Some(false)
            {
                out.push(Out::Tell(change.by, format!("{nick} isn't allowed here.")));
                continue;
            }
            out.extend(nicks.apply(&change, &real, now));
            let shown = change.nick.as_ref().map(|n| format!("~{n}"));
            if game.native_set_nickname(change.uuid, shown.as_deref()) {
                smp.nicks_shown.insert(change.uuid);
            } else {
                smp.nicks_shown.remove(&change.uuid);
            }
        }
        for p in &players {
            if !smp.nicks_shown.contains(&p.uuid)
                && let Some(nick) = nicks.nick_of(p.uuid)
                && game.native_set_nickname(p.uuid, Some(&format!("~{nick}")))
            {
                smp.nicks_shown.insert(p.uuid);
            }
        }
        smp.nicks_shown.retain(|u| players.iter().any(|p| p.uuid == *u) || nicks.nick_of(*u).is_none());
    }
    // The casino: its frames and rounds each tick while one moves (5
    // pictures a second at most), otherwise once a second for the sweep.
    let mut casino = Vec::new();
    let casino_span = horizon_client::spans::guard("smp_casino");
    if let Ok(e) = &mut smp.economy
        && (n % 20 == 0 || smp.casino.animating())
    {
        e.now = now;
        let on: Vec<u128> = players.iter().map(|p| p.uuid).collect();
        let online = (n % 20 == 0).then_some(&on[..]);
        casino = smp_casino_glue::tick(game, &mut smp.casino, e, now, online);
    }
    drop(casino_span);
    let title = smp.board.as_ref().map(|b| b.config.title.clone()).unwrap_or_default();
    let busy = smp.places.as_ref().is_ok_and(|p| !p.pending.is_empty())
        || !smp.trades.open.is_empty()
        || smp.casino.animating();
    BUSY.store(busy, Relaxed);
    save(smp);
    drop(guard);
    for a in sleeps {
        if let smp_presence::Act::Sleep(uuid, ignored) = a {
            game.native_sleep_ignored(uuid, ignored);
        }
    }
    // A vanished player doesn't hold the night up. The reminder is on the
    // action bar, above the hotbar, not in the middle of the screen: sent once
    // a second, it stays up (an action bar line lasts about 3 s), so it never blinks.
    for (uuid, name) in subtitles {
        game.native_sleep_ignored(uuid, true);
        let line = serde_json::json!({"text": smp_vanish::SUBTITLE, "color": "aqua"});
        game.native_run_command(&format!("title {name} actionbar {line}"));
    }
    for (uuid, name) in unvanished {
        game.native_sleep_ignored(uuid, false);
        // An empty line takes the reminder down at once, and leaves any title alone.
        game.native_run_command(&format!("title {name} actionbar \"\""));
    }
    for line in particles.into_iter().chain(casino) {
        game.native_run_command(&line);
    }
    for (uuid, lines) in sidebars {
        game.native_sidebar(uuid, Some(&sidebar(&title, &lines)));
    }
    if let Some((to, (header, footer))) = tab {
        let (header, footer) = (Text::literal(header).with_color(text::GOLD), Text::literal(footer).with_color(text::GRAY));
        for uuid in to {
            game.native_tab_list(uuid, &header, &footer);
        }
    }
    show(game, out);
}

/// One line in the SMP's audit log (`/invsee`'s moves), outside a command:
/// an Err when the log can't be written, or the economy is off.
pub(in crate::game) fn audit(dir: &std::path::Path, entry: serde_json::Value) -> Result<(), String> {
    let mut guard = state(dir.join(smp_store::DIR));
    let smp = guard.as_mut().expect("opened");
    match &mut smp.economy {
        Ok(e) => e.record(entry),
        Err(_) => Err("the audit log is off (economy.json can't be read)".into()),
    }
}

/// A player who has played here, by name (any case): online first, then
/// the economy's accounts and `usercache.json`.
pub(in crate::game) fn find_player(game: &Game<'_>, name: &str) -> Option<(u128, String)> {
    if let Some(p) = game.native_server_players().into_iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
        return Some((p.uuid, p.name));
    }
    let dir = game.native_world_dir()?;
    let guard = state(dir.join(smp_store::DIR));
    let economy = guard.as_ref().and_then(|s| s.economy.as_ref().ok());
    ServerNames::read(&dir, economy).find(name)
}

/// The names a nickname may not copy: the economy's accounts, the server's
/// `usercache.json` (everyone who has joined) and `ops.json`, beside the
/// world's folder. Read for each `/nick` and each second with changes.
struct ServerNames {
    /// (UUID, real name), and whether an operator.
    names: Vec<(u128, String)>,
    ops: Vec<String>,
    /// Operators by UUID, with their level.
    levels: Vec<(u128, u8)>,
}

impl ServerNames {
    fn read(world: &std::path::Path, economy: Option<&Economy>) -> Self {
        let mut names: Vec<(u128, String)> = economy
            .map(|e| e.accounts().into_iter().map(|a| (a.0, a.1)).collect())
            .unwrap_or_default();
        let server = world.parent().unwrap_or(world);
        let list = |file: &str| -> Vec<serde_json::Value> {
            std::fs::read(server.join(file))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        };
        for e in list("usercache.json") {
            if let (Some(name), Some(uuid)) = (e["name"].as_str(), e["uuid"].as_str().and_then(smp_store::parse_uuid))
                && !names.iter().any(|(u, _)| *u == uuid)
            {
                names.push((uuid, name.to_owned()));
            }
        }
        let ops_file = list("ops.json");
        let ops = ops_file.iter().filter_map(|e| e["name"].as_str().map(str::to_owned)).collect();
        let levels = ops_file
            .iter()
            .filter_map(|e| {
                let uuid = e["uuid"].as_str().and_then(smp_store::parse_uuid)?;
                Some((uuid, e["level"].as_u64().unwrap_or(4).min(4) as u8))
            })
            .collect();
        Self { names, ops, levels }
    }
}

impl smp_nick::Names for ServerNames {
    fn known(&self) -> Vec<(Option<u128>, String)> {
        self.names.iter().map(|(u, n)| (Some(*u), n.clone())).chain(self.ops.iter().map(|n| (None, n.clone()))).collect()
    }
    fn find(&self, name: &str) -> Option<(u128, String)> {
        self.names.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).cloned()
    }
    fn name_of(&self, uuid: u128) -> Option<String> {
        self.names.iter().find(|(u, _)| *u == uuid).map(|(_, n)| n.clone())
    }
    fn level(&self, uuid: u128) -> u8 {
        self.levels.iter().find(|(u, _)| *u == uuid).map_or(0, |(_, l)| *l)
    }
}

/// A player's sidebar lines as the game shows them.
fn sidebar(title: &str, lines: &[Line]) -> NativeSidebar {
    let lines = lines
        .iter()
        .map(|l| match l {
            Line::Name(name) => (Text::literal(name.clone()).with_color(text::GREEN).bold(), None),
            Line::Stat { label, value } => {
                let color = match *label {
                    "Balance" => text::GOLD,
                    "Online" => text::GREEN,
                    _ => text::WHITE,
                };
                (
                    Text::literal(*label).with_color(text::GRAY),
                    Some(Text::literal(value.clone()).with_color(color)),
                )
            }
            Line::Gap => (Text::literal(""), None),
            Line::Footer(address) => (Text::literal(address.clone()).with_color(text::AQUA), None),
        })
        .collect();
    NativeSidebar {
        title: Text::literal(title).with_color(text::GOLD).bold(),
        lines,
    }
}

/// The market's prices written now, when they changed (the world closes).
fn flush_book(smp: &mut Smp) {
    if let Some((path, book)) = smp.market.book_to_write(chrono::Utc::now().timestamp_millis(), true) {
        let _one = WRITES.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = smp_store::write_json_compact(&path, &book) {
            log::error!("[smp] {e}");
        }
    }
}

/// The world closed: its files are written and its state let go.
pub fn closed() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(smp) = state.as_mut() {
        save_now(smp);
    }
    *state = None;
}
