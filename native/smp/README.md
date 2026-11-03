# SMP (Horizon native server mod)

The official Horizon server's survival layer ([design](https://github.com/horizon-rust/horizon/blob/main/docs/smp.md)).
It is **not** part of Horizon's own server: only a server that builds this mod into its engine has it. It isn't listed
in the registry.

Players install nothing: it speaks only what a stock Horizon client already shows (commands with completions, chat
lines, vanilla chest menus the server fills with ordinary items (the trade menu); later sounds, titles, the action
bar, boss bars and the sidebar).

This version: the economy, the auction house, the server market and its `/shop` menu, player trades, the coinflip, the lottery, places (spawn, homes, warps,
teleport requests, the rules), land claims, and the scoreboard (a sidebar with your balance, playtime and the richest players, the Tab list's
header and footer).

| Command | Does |
|---|---|
| `/balance [player]`, `/bal` | your balance, or another player's |
| `/pay <player> <amount>` | gives money to a player who has an account (anyone who has played since the mod was on) |
| `/baltop [page]` | the richest players, 10 a page |
| `/eco give <player> <amount> <reason>`, `/money give ...` | operators of level 3: makes money for a player (an event prize, a bug refund); everyone reads it with the reason, and the audit log keeps it |
| `/eco take <player> <amount> [reason]`, `/money take ...` | operators of level 3: takes a cheat's gains out of the game, logged with the reason |
| `/eco stats` | the money supply and where it sits, what the server made and destroyed today, how evenly money is spread |
| `/ah` | the auction house: a chest menu of listings by category, cheapest a unit first; click one to buy it or bid; your listings (click to take one back) and your mail |
| `/ah sell <price> [count]` | lists the stack in your hand (or part of it), whole (enchantments, name, contents...), at a fixed price, for `listing_hours` (48); listing is free |
| `/ah auction <start> [hours]` | auctions the stack in your hand from a starting bid, for 1 to 24 hours (12) |
| `/ah bid <listing> <amount>`, `/ah buy <listing>` | bids (the money is held until someone bids more or it ends), buys a fixed-price listing |
| `/ah cancel <listing>`, `/ah claim`, `/ah search <words>`, `/ah mine`, `/ah mail` | takes back a listing nobody bid on; gives you your mail; the menu on a search, your listings, your mail |
| `/shop` | the market as a chest menu: categories, a click buys one, a shift-click 64, a click on a stack of yours sells it, "Sell all" |
| `/market [item]` | the moving market as a chest menu: categories, each item's price, its move over 1 h and 24 h and its volume; an item's page buys or sells 1, 16 or 64, sells all you carry, and gives its price chart as a map |
| `/worth [item]` | an item's price, the same to buy and to sell (the one in your hand without a name), and what you may still sell today |
| `/sell hand [count]`, `/sell <item> [count]`, `/sell all` | sells to the market: the stack in your hand, an item, or everything it buys |
| `/buy <item> [count]` | buys from the market, only what fits in your inventory |
| `/trade <player>` | asks a player to trade (60 s to answer) |
| `/trade accept [player]`, `/trade deny [player]` | answers a request |
| `/trade money <amount>` | the money you add to your offer (0: none) |
| `/trade cancel` | ends your trade, or withdraws your request |
| `/coinflip <amount> [player]` | stakes an amount for anyone (or one player) to match; off unless `gambling.enabled` |
| `/lottery`, `/lottery buy <tickets>` | the pot, your tickets and the time to the draw; buys tickets (off unless `gambling.enabled`) |
| `/casino`, `/casino roulette\|slots\|crash\|blackjack` | the casino's lobby menu, or a game's table (needs `gambling.enabled` and `casino.enabled`) |
| `/casino help`, `/casino top`, `/casino stats` | every game's odds and house edge and the limits; today's biggest wins; what you bet, won and may still lose today |
| `/coinflip accept <player>`, `/coinflip cancel`, `/coinflip list` | takes an offer (a fair coin decides, the winner gets both stakes), withdraws yours, lists those waiting |
| `/spawn` | to a safe spot within `places.spawn_radius` of the server's spawn; `/setspawn` (operators of level 3) sets it where you stand |
| `/home [name]`, `/sethome [name]`, `/delhome [name]`, `/homes` | your homes (`home` when unnamed): go, set or move one where you stand, delete, list |
| `/warp <name>`, `/warps` | to a warp, the list; `/setwarp <name>`, `/delwarp <name>` for operators of level 3 |
| `/tpa <player>`, `/tpaccept [player]`, `/tpdeny [player]` | asks to go to a player (60 s to answer); answers a request |
| `/rules` | the server's rules |
| `/back` | back to where you died, or where your last teleport took you from (warmup and cooldown as any teleport) |
| `/afk` | marks you away from the keyboard until you move, turn or run a command |
| `/playtime [player]` | how long you (or a player who is on) have played here |
| `/claim`, `/claim info`, `/unclaim` | claims the chunk you stand in (all its height), says whose it is, frees it (operators of level 3: anyone's) |
| `/claim show` | draws the border of the chunk you stand in for 10 s, for you only: green yours or a trusting owner's, red someone else's, white free |
| `/claims` | your chunks, how many you may hold, and who you trust |
| `/trust <player>`, `/untrust <player>` | lets a player (or no longer) build and use things on all your land |
| `/nick <name>`, `/nick off`, `/nick` | operators of level 3 (with `ops_only`, the default): another name for you in chat and the Tab list, shown as `~name` (hovering it gives your real one); `/nick off` (anyone with a nickname) takes it off; `/nick` alone says how |
| `/nick <player> <name\|off>` | operators of level 3: sets or takes off a player's nickname |
| `/realname <nick>` | who goes by that nickname |
| `/invsee <player>`, `/endersee <player>` | operators of level 3: the player's inventory (main, hotbar, armour, off hand) or ender chest in a chest menu, on or off the server; click one of theirs to take it, one of yours to give it; every move in `audit.log`, a summary in `staff.log` |
| `/sidebar off`, `/sidebar on`, `/sidebar` | hides the scoreboard sidebar for you (remembered), shows it again |

The market trades only plain items (not worn, named or enchanted), and only with players in survival or adventure
mode. Items move first, then money, both logged; a sale the log refuses gives the items back, and a purchase checks the
money and the room before anything moves.

**No way to get rich from the market.** The market makes money only from what players gather, and only so much a day:
- One price per item, to buy and to sell (since 0.29): a config that still has two (`buy` 400, `sell` 100) gets
  their geometric mean (200) at load, logged ("one price now"); `buy` or `sell` 0 still closes that side. A trade
  of N items is priced along the curve it moves the price on (below), so buying N and selling them back gives back
  exactly what was paid (less whole-coin rounding): a round trip never earns (tested for 1, 64 and past the minute's band, and
  over thousands of random trades with the drift in between).
- No chain of recipes turns what the market sells into something it buys for more: the cheapest way to get each item
  from the market (bought, or crafted, smelted, cut from bought items, through any number of recipes) is worked out,
  and an item the market would buy for more than that isn't bought. The tests walk every vanilla recipe
  (`SMP_VANILLA_DATA=<.local/horizon-client/26.3/data/minecraft> cargo test`); the server checks the iron, gold,
  copper, diamond, emerald, coal, redstone, lapis, wheat, wood and food chains at each start.
- Items a recipe links (an ingot and its block, wheat and hay...) share one moving price, the block moving it as
  its 9 ingots do, so crafting never beats the market however prices move; the vanilla-recipe test checks every item
  at the worst drift (what makes it at 0.6 times its price, it at 1.6) too.
- Each player sells at most `daily_sell_coins` (2,000 coins) and `daily_sell_items` (640 of each item) a UTC day,
  `/worth` and the shop show what is left, and `market.json` keeps the day's count across restarts.
- Money received with `/pay` counts toward the receiver's same `daily_sell_coins`: selling up to the limit on alts
  and paying it all to a main is refused once the main's limit is reached ("can only receive N more today").
  Trades (`/trade`) and auction house sales are not counted.
- Amounts are checked: no price above 1,000,000,000,000, no sum that overflows.
- The coinflip and the lottery give the winner all the stakes: they move money between players and never make any.

Default prices (coins; "buy only": the market sells it but doesn't buy it, "sell only" the other way):

| Category | Items |
|---|---|
| Farming | wheat 2, carrot 2, potato 2, beetroot 2, sugar cane 2, pumpkin 5, melon slice 1, cactus 2; buy only: wheat seeds 2, bone meal 3 |
| Mining | coal 4, copper ingot 3, iron ingot 15, gold ingot 20, redstone 2, lapis 4, quartz 4, emerald 40, diamond 200 |
| Wood | the eight logs 2 |
| Mob drops | bone 2, string 2, gunpowder 4, spider eye 2, slime ball 4, blaze rod 15, leather 3, feather 1; sell only: rotten flesh 1 |
| Building (buy only) | cobblestone 1, stone 2, dirt 1, sand 2, gravel 2, glass 3, bricks 4, stone bricks 3 |
| Food (buy only) | bread 5, cooked beef 6, cooked porkchop 6, cooked chicken 5, baked potato 4, apple 4, golden carrot 40 |
| Tools (buy only) | torch 1, ladder 2, crafting table 4, chest 6, furnace 6 |

A `config.json` still holding the 18 prices of versions 0.1 to 0.11, never edited, gets these at the first start of
0.12.0; edited prices are kept. Only items that stack to 64 belong in the market: a shift-click buys 64, and what
doesn't fit falls at the player's feet for them.

**/invsee and /endersee.** For operators of level 3 (luzW, bunchy7s...). The menu shows the main inventory in its
first three rows, the hotbar in the fourth, then the armour (head to feet) and the off hand; the ender chest fills
three rows. A click on one of the player's stacks moves it whole into the operator's inventory; a click on one of the
operator's own moves it into the player's (main inventory or ender chest, where it fits whole). A stack moves only if
it is still exactly what the menu showed: the game checks it and moves it, or nothing (no copy, no loss). A player who
isn't on is read from and written to their saved file; while they log in or are dead nothing moves and the menu says
why. The menu is read again once a second while open; nothing runs when none is open. Each move is in `audit.log`
before it is made (`invsee_take`, `invsee_put`: operator, player, inventory, item, count; a refused one again with
its reason); closing the menu writes one line to the server log (`[smp] staff: ...`), `staff.log`, and the chat of
the other operators on. Needs horizon#6106 (`native_inventory`, `native_move_stack`).

**Nicknames.** With `ops_only` (the default since 0.22) only operators of level 3 set nicknames: their own, or a
player's with `/nick <player> <name>`; a player may take off the one given to them (`/nick off`). At each start, a
nickname a player who isn't an operator picked themselves is taken off and logged (`for operators only`). 3 to 16 letters, digits or `_`, shown with a `~` in front (`~Rocky`) so a nickname never passes for
a real name. Names are compared case-insensitively with `_` dropped and look-alikes folded (`0`→o, `1`/`i`→l, `3`→e,
`4`→a, `5`→s, `7`→t, `8`→b, `9`→g). Refused: a nickname equal to, or containing (from 4 letters), the name of anyone
who has played here (the economy's accounts and `usercache.json`), an operator (`ops.json`), the Team (`protected`)
or another player's nickname; one that reads as a reserved word (`reserved`: server, console, admin, mod, owner,
staff, op, horizon, kira, notch, mojang, herobrine...); one the server's chat filter refuses. A player needs
`min_playtime_minutes` of play before their first nickname (operators don't). At each start, nicknames that break
these rules are taken off, each logged (`[smp] nickname X of <uuid> taken off: why`). The change is made at the next
second. The nickname shows in chat (messages, `/me`, `/msg`) and the Tab list, and hovering it gives the real name;
the real name stays everywhere else: the server's log, the anticheat, selectors, the Discord bridge. `/pay`, `/tpa`,
`/trade`, `/playtime`, `/tpaccept`, `/tpdeny` and `/eco` take a nickname (with or without `~`) wherever they take a
player's name. The player's profile (name, UUID) never changes.
`config.json`'s `nicks`: `enabled`, `ops_only` (true), `cooldown_seconds` (600), `min_playtime_minutes` (60), `protected` (the Team's
names), `reserved` (the words above).

**Moving prices** (`market.dynamic`, on by default since 0.26). Each item has one price: its base price (`items`)
times a factor that supply and demand move as each trade happens. Every item bought raises the log of the factor by
`1% / depth`, every item sold lowers it as much, where the depth follows the item's rarity (400 / its price, between
2 and 400: a diamond moves 50 times faster per item than cobblestone; `depth` overrides it per item). A trade of N is
worth the area under that curve (`price / step × (e^(N × step) - 1)`), rounded up to whole coins when buying and down
when selling: one diamond at 200 costs 201 and sells for 199, 64 cobblestone at 1 cost 65, and a sale walks back the
exact path of the purchase. All trades together move an item at most `max_minute_move_percent` (5%) a minute from
where it started that minute: past that, a purchase is cut short and a sale keeps the rest ("sell them in a
minute"). The factor goes back to 1 with a half-life of `half_life_hours` (6) and stays within `min_factor` (0.6)
and `max_factor` (1.6), where the rest of a trade goes at that price. There is no tax on trades (an older config's
`tax_percent` is ignored): the receipt is the price ("Sold 3 diamond at 198 coins each: 595 coins."). Items a player bought from the market in the last `round_trip_hours` (24) can't be
sold back to it. (`max_trade_impact_percent` and `max_player_impact_percent` are no longer used: the curve and the
minute's band do their work.) History: one point a minute for 24 hours and one an hour for 30 days (the mid price, the
items traded and those sold), in `market_prices.json`, written by a thread every 5 minutes and when the world closes.
At the first minute of each UTC day, the day's three biggest risers and fallers (at least 1%) and news from real
activity ("Diamond supply up 40% today", sales against the day before's, at least `news_percent` and
`news_min_volume`) go to everyone's chat and to `web/market.json` for Kira to post. `enabled: false` keeps the
fixed prices of 0.23. For other mods: `Market::spot`, `volume(id, now, hours)`, `history(id, Span)`.

**`/market`.** A 6-row chest menu: the shop's categories and the day's movers; a category's items, each with its
price, its change over 1 h and 24 h (green ▲, red ▼) and what was traded in 24 h; an item's page with its
base price and what you may still sell today, buttons to buy or sell 1, 16 and 64 (what they come to
along the curve), sell all of it you carry, and the price chart over 24 hours or 7 days, given as a filled map (128 × 128,
drawn by the server: the curve green when it rose, red when it fell, the price, the change, the low and high). A
chart map is drawn again each minute for whoever carries it, and once a minute the open menus follow the prices.
`/market <item>` opens the item's page. `/worth` adds the item's moves and volume.

**The shop.** `/shop` opens a 6-row chest menu on the market's prices: the categories of `config.json`'s `shop` (an
item in none goes under "Other"), each item named with its prices. A click buys one, a shift-click 64; a click on a
stack in your own inventory, below the menu, sells it; "Sell all" sells everything the market buys. The bottom row
shows your balance and today's selling limit. It goes through the market's own rules (survival or adventure only,
items then money, logged, the daily limits); the items move by the console's `clear` and `give`, measured on the
inventory before and after, so the money follows what really moved.

**Trades.** Once accepted, both players see a 6-row chest menu: their own offer on the left (16 stacks), the other's
on the right, each side's money, their confirm and cancel buttons, and whether the other has confirmed. A click on a
stack in your inventory, below the menu, adds it to your offer; a click on it in your offer takes it back. The menu
moves nothing: offered stacks stay in their owners' inventories, the menu shows copies, and a stack that changes
(used, dropped, grown by a pickup) leaves the offer. Any change clears both confirmations. When both have confirmed,
the server checks that each still holds exactly what they offer, has room for what they get and has the money, logs
the trade, and swaps everything in one step, or nothing (and both must confirm again). Closing the menu,
`/trade cancel`, leaving, dying or the other player leaving ends the trade with nothing moved. Survival and
adventure players only.

**Coinflips.** Off by default: the owner turns them on in `config.json` (`gambling`: `enabled`, `min_bet` 10,
`max_bet` 1000, `cooldown_seconds` 30, `offer_seconds` 60). A stake is held from the offer, in
`economy.json` with the balances, until the coin falls; an offer cancelled, not taken in time, or whose player
left gives it back, and so does the next start for one the server stopped with. The coin is a fresh random key from
the system, never the world's random. Only the in-game currency is staked and won.

**The lottery.** Part of `gambling` (`ticket_price` 10, `max_tickets` 100 a player a draw, `lottery_seconds` 3600
from a round's first ticket to its draw). Tickets are held like coinflip stakes; at the draw one ticket is picked at
random among all sold (more tickets, more chances), its owner gets the whole pot, and everyone hears
it. A round with a single player, or one the server stopped with, gives every ticket back.

**The casino.** On with `gambling.enabled` (and `casino.enabled`, true by default). Every game is a chest menu any
stock client can play, animated by changing its items at most 5 times a second for each open menu (and only while a
round moves), with note-block sounds, titles and the action bar for results, fireworks and a line for everyone on a
jackpot:

- European roulette: one zero, red/black, odd/even, 1-18/19-36 pay 1 to 1, dozens and columns 2 to 1, a number 35 to
  1; house edge 2.70% on every bet. The wheel turns and slows down on the drawn pocket.
- Slots: 3 reels of 32 stops with item symbols, one payline, a published paytable (three Nether Stars 500x down to
  three Berries 6x, two Berries 2x); 94.2% paid back (house edge 5.8%), one pull in 4.5 pays.
- Crash: one shared round; bet in the 10 s before take-off, then cash out (or set an automatic cash-out) before it
  crashes. P(crash at x or more) = 96% / x, capped at 100x: 96% paid back whatever you do. Each player in the round
  sees the multiplier in their own boss bar and the action bar.
- Blackjack: 6 decks shuffled each hand, the dealer stands on all 17s and peeks, blackjack pays 3 to 2, double on the
  first two cards, no split; about 1% house edge with basic strategy.

Every draw is made by the server (ChaCha20 keyed from the system) before the animation starts. Bets go from `min_bet`
10 to `max_bet` 1000; a player may lose at most `daily_loss_cap` (10,000) a day (UTC, all games together) and win at most
`daily_win_cap` (10,000) net (once reached, no new bet until 00:00 UTC; the round that crosses it is paid in full), and
`cooldown_ms` (2000) passes between the end of a round and their next bet. A bet is held like a coinflip stake;
closing the menu or leaving never loses a round: a spin or a pull ends and is paid, a blackjack hand stands, a crash
bet is cashed out when its player leaves in flight (or given back before take-off). Wins are paid from the casino's
pot (`economy.json`'s pool `casino`): a win larger than the pot is paid up to it. A new casino's pot gets `float`
(20,000) once, made for it (`casino_float`; an older pot only what it lacks of it), then keeps every lost bet: the
house edge stays in the pot, out of the players' hands. A win of `jackpot_multiple` (50) times the stake, or of `announce_profit` (5000) coins, is
told to everyone. Settings in `config.json`'s `casino`, also `crash_edge_percent` (4) and `crash_max` (100).

**Places.** A teleport waits `warmup_seconds` (3) in `config.json`'s `places`; a player who moves or is hurt meanwhile
stays where they are, and `cooldown_seconds` (10) passes between two of one player's teleports. Teleports are free (an older config's
`places.fees` is ignored). `/tpa` takes the
asker to where the other player is when the warmup ends. Homes grow with the time played on the server since the mod
was on (`homes`: `[hours, homes]`, by default 1 home, 2 from 5 hours, 3 from 20, 5 from 100). `rules` holds `/rules`'
lines. `/spawn` lands on a random safe column within `spawn_radius` (250) blocks of the spawn point, like vanilla's `respawn_radius` for a first join or a respawn: after the `tp`, vanilla's `spreadplayers` moves the player off fluid, fire and magma onto solid ground (0: exactly the spawn point; if no column is found the player stays at the spawn point). Teleports are vanilla's `tp`, run as the server (`execute in <level> run tp`), so they cross levels and land
as any teleport does.

**Claims.** In a claimed chunk, only its owner, the players they trust and operators of level 3 break blocks or right
click blocks: doors, chests, levers, buttons, placing a block, a bucket or flint on a block. Anyone else's click does
nothing, and the action bar says whose land it is. The chunks a player may hold grow with the time played here
(`claims.chunks`: `[hours, chunks]`, by default 4, 16 from 5 hours, 36 from 20, 64 from 100); `claims.enabled`
turns them off. `claims.free_radius` (0, none) keeps the land within that many blocks of 0 0, in every dimension, free
for all: no one claims a chunk that touches it, and claims already there are dropped at start (each logged). The world stays out too: no water or lava flows, no fire spreads or burns, and no piston pushes into
a claim from land that isn't its owner's (or a player's they trust), and explosions break no claimed block, whoever
lit them. Buckets are guarded where they fill or empty, and hitting the claim's item frames, armour stands,
paintings, boats, minecarts and animals does nothing for others (`claims.protected_entities`, entity ids; monsters
and players aren't in it). Not guarded yet: right clicking an entity (rotating a frame's item, an armour stand's
items, shearing, leads), which waits for Horizon's hook (#4648). The server's `spawn-protection` (vanilla's) guards
the spawn.

**The scoreboard.** A sidebar on the right, titled `board.title` ("Horizon SMP"): your own block first (your name, or
your nickname; your balance, `125.4k`, `3.2M`; your playtime, `2h 05m`; the players online), then the `board.top` (3)
richest players with their balance, then `board.footer` (`play.playhorizon.dev`, empty for none). The ranking is
built once for everyone, only when a balance changed (at most every `board.min_seconds`, 5) or every
`board.refresh_seconds` (60); each player's own lines are looked at on the same `board.refresh_seconds`, and a
sidebar is sent only to the players whose lines changed and to newcomers: nothing runs per tick per player. The Tab
list shows `board.tab_header` and `board.tab_footer` (`{server}`, `{tps}`, `{players}` filled in, every
`board.tab_refresh_seconds`, 5). `board.sidebar` and `board.tab` turn either off; `board.server_name` names the Tab
list. A server that kept 0.13's defaults (`Richest players`, the top 10) gets these at its first start of 0.14. The
sidebar uses one scoreboard objective of its own (`horizon:native_sidebar`) and leaves the world's scoreboard alone.

**Presence.** A player's first join is announced to everyone (`presence.first_join`, empty for none) and they read
`presence.welcome` and the rules. Each player gets `presence.daily_reward` (50) once a day (UTC) after
`presence.reward_after_minutes` (10) of play that day, logged as `daily_reward` (0 turns it off). Only seconds with a
move, a turn or a command in the minute before count, so an alt that logs in and stands still gets nothing (on
2026-11-01, 1,242 players got the reward; 323 of them had played 10 minutes in all). The count isn't kept over a
restart. A player who hasn't moved, turned or run a command for `presence.afk_minutes` (5),
or typed `/afk`, is AFK: they don't count for sleeping (vanilla's `players_sleeping_percentage`) until they move or
get in a bed; `presence.afk_announce` tells everyone instead of only them. A player runs at most
`presence.spam_commands` (8) of the SMP's commands in `presence.spam_seconds` (5); operators of level 3 aren't
limited. All of it is looked at once a second.

**The auction house.** Players sell to players: a listing is a fixed price or a timed auction. The listed items
leave the seller's inventory at once; what doesn't sell, what an auction's winner gets, and what didn't fit in a
buyer's inventory wait in the player's mail (`/ah claim`, or the menu's mail page; as much as fits, the rest stays).
A purchase takes the whole listing: the listing leaves first, the items move, then the money (the seller gets the
whole price); of two players who buy at once (the game runs every click on one
thread), the second finds it gone. Bids wait in the `auction` pool: a higher bid gives the last one back at once,
raising one's own only holds the difference, and a bid in the last `snipe_seconds` (60) moves the end back to
60 seconds away. When an auction ends, the seller is paid the winning bid and the items go to the winner's mail. Sellers
and outbid players who are offline find a note when they come back. Everything (listings, bids, mail, notes) is
written in `economy.json` with the balances, in one step: a crash can't keep the money and lose the items, or the
reverse. A listing carries the whole stack (since 0.31): its enchantments, name, damage, lore, a shulker box's
or a bundle's contents and every other component, written as the game writes it in player files, and given back
exactly so (Horizon's `native_give_stack`) to the buyer, the winner, or the seller when it comes back. `/ah sell
<price> <count>` lists part of the stack in hand; the rest stays in the hand, the same. The menu shows the stack
itself (its tooltip), with the listing's price, seller and time under it. A stack is taken only if it would come
back the same (its data written and read back equal); not market charts (free at `/market`), nor a stack holding
more than 32 KiB of data (a box of books). Listings and mail from before 0.31 hold plain items and are given back as
before. Survival and adventure players only. `config.json`'s `auction`:
`enabled`, `max_listings` (8 a player), `listing_hours` (48), `auction_hours` (12),
`auction_hours_max` (24), `bid_step_percent` (5: the smallest raise), `snipe_seconds` (60). Ended listings are
looked at once a second (a pass over the open listings).

**Money in and out.** Money enters the game through what the market buys from players, the daily reward, the
casino's float (once) and operators' `/eco give` (with a reason everyone reads, logged as `eco_give`). It leaves
through what the market sells and operators' `/eco take`. There are no fees, taxes, bank, contracts or futures: no
VAT, land or wealth tax, no tax on market trades, no teleport, auction or coinflip fee. A world from before 0.30 is
cleaned up at its first start: bank savings and the margins of open futures positions go back to their owners
(`bank_closed`, `futures_closed`), and the Treasury, its insurance fund, contracts and land tax owed are dropped.
`config.json`'s old `taxes`, `bank`, `jobs` and `futures` sections and the `fee_percent` and `listing_fee_percent`
keys are ignored.

**The report.** `/eco stats` (anyone) shows the money supply (wallets and money held, each pool, the casino's pot),
what the server made today (market sales, the daily reward, `/eco give`) and destroyed (market purchases, operators'
`/eco take`), and how evenly money is spread (Gini over players
with money, the richest ten's share). At 00:00 UTC everyone reads the day before in one line (`report.daily_report`).
A snapshot of each day is kept 60 days (`report.history_days`) in `economy.json` (`ext.report`). Once a minute
`world/horizon-smp/web/economy.json` (`report.web_json`) is written off the tick thread for the site's /hub:
`{"schema": 1, "generated": <ms>, "currency", "today", "pools", "minted_today", "burned_today", "days": [...],
"daily_report": "<yesterday's line>"}`. The server's Discord relay posts only chat, joins, leaves and deaths: the
daily line reaches Discord through whoever reads that file (Kira).

Other parts reach the pools through `Economy::to_pool`, `from_pool`, `pool_move` and `settle_pool` (a bet against a
pool: the stake goes in, the payout comes out, never more than the pool holds), each one audit line.

The currency is earned in game only: it can't be bought with real money or cashed out (Mojang's usage guidelines).

## Data

Everything is in the world's folder, `world/horizon-smp/`, so a backup of the world carries it. A world without that
folder is untouched until the mod first runs on it; it changes no block and no player.

| File | Holds |
|---|---|
| `config.json` | `currency` (default `coins`), `starting_balance` (default 0), `market`: `enabled`, `items` (each item's `buy`, what the market asks, and `sell`, what it pays, 0 closing that side), `daily_sell_coins` (2000) and `daily_sell_items` (640), 0 for no limit; `shop`: `enabled`, `title`, `categories` (`name`, `icon`, `items`); `web`: `url` (Kira's `/smp/ingest`) and `token` (empty: nothing is sent), where the pages of `web/` go; written with the defaults the first time, and again when a newer version adds keys (your values kept) |
| `economy.json` | balances by player UUID, with the name last seen; `held`: coinflip stakes and lottery tickets being decided; `pools`: the casino's pot and the auction house's bids; `ext`: other parts' state written in the same step as the money (`auction`: the listings, bids, mail and notes) |
| `places.json` | the spawn, the warps, each player's homes, and the seconds each player has played (written once a minute) |
| `nicks.json` | each player's nickname and when they last changed it |
| `staff.log` | one line per `/invsee` or `/endersee` menu in which something moved: who, whose, what was taken and put |
| `market.json` | what each player sold today (coins and items), for the daily limits |
| `market_prices.json` | each item's price factor and history (minutes for 24 h, hours for 30 days: `[time, mid, traded, sold]`), what each player bought in the last day (no selling it back), the day's summary |
| `web/market.json` | for the website (`/hub/market`, through Kira), written by a thread each minute prices move: `schema` 1, `generated`, `currency`, `tax_percent` (always 0, for older readers), `daily_summary` (`day`, `lines`), `items` (`id`, `name`, `category`, `price` (the one price; `buy` and `sell` repeat it on the sides the market trades, for older readers), `mid`, `base`, `change_1h`, `change_24h`, `volume_24h`, `day`: 5-minute points, `month`: hours, each `{step, t, price, volume}`). When `web.token` is set, the same thread POSTs each page of `web/` that changed (gzipped, `Authorization: Bearer <token>`) to `<web.url>/<name>`: Kira keeps the last one and serves it to the site; the server needs no inbound access and Kira no key to it |
| `presence.json` | the players who have joined (no welcome again), and the day each last got the daily reward |
| `board.json` | the players who hid the sidebar (`/sidebar off`) |
| `casino.json` | the day's ledger: each player's bets, wins and rounds today (the daily loss cap), the day's biggest wins, each game's totals, the pot's last sweep |
| `claims.json` | each claimed chunk (`<dimension> <chunk x> <chunk z>`) with its owner, and whom each owner trusts |
| `audit.log` | one JSON line per change, written before the change: `time` (ms), `kind` (`open`, `pay`, `grant`, `ah_listing_fee`...), `amount`, `from`/`to` with their balance before, `by` for an operator, `to_pool`/`from_pool` with `pool_before` and `reason` for a pool's movement; a trade's line (`trade`: both players, their items and money) comes before its swap, `trade_failed` after one that didn't go through |

`economy.json` is written whole to a temporary file, flushed and renamed over the old one, at most once a server tick
and when the world closes: a crash leaves the old file or the new one. If it can't be read, the economy stays off (the
commands say so) rather than start from nothing.

## Made on the hook points

It hooks only Horizon's native hook points: `native_register_commands` and `native_perform_command` (API 8), the
acting player's items (API 9), the server menus and `native_exchange_items` (API 10), the players' health and
`native_run_command` (API 21), `native_destroy_block` (API 14), `native_right_click_block`, the acting player and the
action bar (API 21), `native_block_spread`, `native_piston_move`, `native_explosion_detonate`, `native_fill_bucket` and
`native_attack_entity` (API 22), `native_sidebar`, `native_tab_list` and `native_tick_stats` (API 49),
`native_set_nickname` and `native_text_allowed` (API 62), the server tick and the world lifecycle.
`compatibility.json` names `"hooks": 58`.

| File | What it is |
|---|---|
| `mixins/smp/horizon-mixin.toml` | The bundle: twenty-seven modules and the HEAD hooks |
| `mixins/smp/smp_store.rs`, `smp_economy.rs`, `smp_market.rs`, `smp_recipes.rs`, `smp_shop.rs`, `smp_nick.rs`, `smp_trade.rs`, `smp_games.rs`, `smp_lottery.rs`, `smp_places.rs`, `smp_claims.rs`, `smp_board.rs`, `smp_presence.rs`, `smp_auction.rs`, `smp_report.rs`, `smp_market_prices.rs`, `smp_market_menu.rs`, `smp_market_chart.rs`, `smp_invsee.rs` | The rules, no game types (a trade's stacks are a type parameter): added to the game as `game::smp_store`, `game::smp_economy`, `game::smp_market`, `game::smp_recipes`, `game::smp_shop`, `game::smp_nick`, `game::smp_trade`, `game::smp_games`, `game::smp_lottery`, `game::smp_places`, `game::smp_claims`, `game::smp_board`, `game::smp_presence`, `game::smp_auction`, `game::smp_report`, `game::smp_market_prices`, `game::smp_market_menu`, `game::smp_market_chart`, `game::smp_invsee`; also this crate's modules (`src/lib.rs`), for their tests |
| `mixins/smp/glue.rs` | The state for the open world and the edges to the game's types (`game::smp_glue`) |
| `mixins/smp/{register,command,tick,lifecycle,menu_clicked,menu_closed,destroy_block,right_click_block,block_spread,piston_move,explosion_detonate,fill_bucket,attack_entity}.rs` | One small helper per hook |
| `mixins/smp/smp_casino.rs`, `smp_casino_roulette.rs`, `smp_casino_slots.rs`, `smp_casino_crash.rs`, `smp_casino_blackjack.rs` | The casino's rules, menus and effects (`game::smp_casino*`) |
| `mixins/smp/casino_glue.rs` | The casino's effects for a stock client: menu items, sounds, titles, fireworks, boss bars (`game::smp_casino_glue`) |
| `proof.py` | A dedicated server built with the mod and two real clients, on fresh temporary data |
| `proof_casino.py` | The casino's proof (`SMP_PROOF=casino`): both clients play every game through the menus |

## Build and prove

```sh
(cd native/smp && cargo test)                          # the rules
python3 native/tools/native.py check smp               # apply on the tested Horizon and cargo check the game
python3 native/tools/native.py proof smp --resources /path/to/.local/horizon-client/26.3
```

## The official server's engine

The server runs `horizon-server` as usual, with `[engine] path` pointing at a game built with this mod:

```sh
NATIVE_NO_DEFAULT_FEATURES=1 python3 native/tools/native.py build smp --horizon <the release's commit>
```

prints the built `horizon-client` (`--no-default-features`: the server build, without audio and controllers). When
to deploy it on the official server is the owners' decision.
