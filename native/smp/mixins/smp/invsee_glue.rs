// `/invsee` and `/endersee` in the server's game (smp_invsee's rules): the
// menus open, their clicks and closings, the inventories through the native
// helpers (`native_inventory`, `native_move_stack`), each move in the SMP's
// audit log, the staff's summary in the server log, `staff.log` and the
// chat of the other operators on. Its own state, apart from the SMP's: the
// tick costs one atomic load while no menu is open. Added to the game as
// `game::smp_invsee_glue`.

use crate::game::{
    Game,
    native_inventories::NativeInventoryKind,
    native_menus::native_menu_inventory_slot,
    native_server::{NativeCommand, NativeServerPlayer},
    smp_economy::{Out, Player},
    smp_glue,
    smp_invsee::{self, Cell, Invsee, Kind, Move},
    smp_store, smp_switch,
};
use horizon_client::{
    items::ItemStack,
    lan::data::component_nbt,
    text::{self, Text},
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
};

struct State {
    invsee: Invsee<ItemStack>,
    /// Asked in a command, opened at the next tick: (operator, target UUID,
    /// real name, which).
    pending: Vec<(u128, u128, String, Kind)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// A menu open or one to open: the tick has work.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static TICKS: AtomicU64 = AtomicU64::new(0);

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    s.get_or_insert_with(|| State {
        invsee: Invsee::default(),
        pending: Vec::new(),
    });
    s
}

impl smp_invsee::Stack for ItemStack {
    fn id(&self) -> String {
        self.item.to_string()
    }
    fn count(&self) -> u32 {
        self.count
    }
}

fn native(kind: Kind) -> NativeInventoryKind {
    match kind {
        Kind::Player => NativeInventoryKind::Player,
        Kind::Ender => NativeInventoryKind::Ender,
    }
}

/// The game's inventories, for the rules.
struct GameWorld<'a, 'g>(&'a mut Game<'g>);

impl smp_invsee::World<ItemStack> for GameWorld<'_, '_> {
    fn read(&self, uuid: u128, kind: Kind) -> Result<(Vec<Option<ItemStack>>, bool), String> {
        self.0.native_inventory(uuid, native(kind)).map(|i| (i.slots, i.online))
    }
    fn move_stack(&mut self, from: (u128, Kind, usize), expected: &ItemStack, to: (u128, Kind)) -> Result<(), String> {
        self.0.native_move_stack((from.0, native(from.1), from.2), expected, (to.0, native(to.1)))
    }
}

pub fn commands(commands: &mut Vec<NativeCommand>) {
    for usage in ["invsee <player:word>", "endersee <player:word>"] {
        commands.push(NativeCommand::op(usage, smp_invsee::LEVEL));
    }
}

/// `/invsee` and `/endersee`; anything else is left to the SMP.
pub(in crate::game) fn perform(game: &mut Game<'_>, player: &NativeServerPlayer, line: &str, ran: &mut bool) {
    let word = line.split(' ').next().unwrap_or_default();
    if !smp_invsee::COMMANDS.contains(&word) || !game.native_on(smp_glue::ID) {
        return;
    }
    *ran = true;
    if !smp_glue::part_on(game, smp_switch::Part::Invsee) {
        let refusal = smp_glue::refusal(smp_switch::Part::Invsee);
        game.native_feedback(Text::literal(refusal).with_color(text::RED));
        return;
    }
    let me = Player {
        uuid: player.uuid,
        name: player.name.clone(),
        level: player.permission,
    };
    let find = |name: &str| smp_glue::find_player(game, name);
    let mut guard = state();
    let s = guard.as_mut().expect("made");
    let (out, open) = s.invsee.command(&me, &find, line).unwrap_or_default();
    if let Some((target, name, kind)) = open {
        s.pending.retain(|p| p.0 != me.uuid);
        s.pending.push((me.uuid, target, name, kind));
        ACTIVE.store(true, Relaxed);
    }
    drop(guard);
    for o in out {
        match o {
            Out::Error(e) => game.native_feedback(Text::literal(e).with_color(text::RED)),
            Out::Reply(r) => game.native_feedback(Text::literal(r)),
            _ => {}
        }
    }
}

fn named(item: &str, name: &str, color: &str) -> ItemStack {
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

fn items(invsee: &Invsee<ItemStack>, op: u128) -> Vec<Option<ItemStack>> {
    invsee
        .layout(op)
        .into_iter()
        .map(|c| match c {
            Cell::Empty => None,
            Cell::Stack(s) => Some(s),
            Cell::Filler => Some(named("minecraft:gray_stained_glass_pane", " ", "gray")),
            Cell::Label(l) => Some(named("minecraft:light_gray_stained_glass_pane", l, "gray")),
            Cell::Close => Some(named("minecraft:barrier", "Close", "red")),
            Cell::Info { name, online, busy } => {
                let line = match &busy {
                    Some(b) => format!("{name}: {b} (nothing moves now)"),
                    None if online => format!("{name} (online): click theirs to take, yours to give"),
                    None => format!("{name} (offline, their saved file): click theirs to take, yours to give"),
                };
                Some(named("minecraft:player_head", &line, if busy.is_some() { "red" } else { "yellow" }))
            }
        })
        .collect()
}

fn title(name: &str, kind: Kind) -> Text {
    Text::literal(match kind {
        Kind::Player => format!("{name}'s inventory"),
        Kind::Ender => format!("{name}'s ender chest"),
    })
}

/// The audit line of a move: written before it is made ("asked"), and
/// again when the game refused it.
fn audit(dir: &std::path::Path, m: &Move, outcome: &str) -> Result<(), String> {
    smp_glue::audit(
        dir,
        serde_json::json!({
            "kind": if m.take { "invsee_take" } else { "invsee_put" },
            "op": smp_store::uuid_string(m.op),
            "target": smp_store::uuid_string(m.target),
            "inventory": match m.kind { Kind::Player => "inventory", Kind::Ender => "ender_chest" },
            "item": m.item,
            "count": m.count,
            "outcome": outcome,
        }),
    )
}

/// The staff's line: the server log, `staff.log`, the other operators on.
fn staff(game: &mut Game<'_>, op: u128, line: &str) {
    log::warn!("[smp] staff: {line}");
    if let Some(dir) = game.native_world_dir() {
        use std::io::Write;
        let path = dir.join(smp_store::DIR).join("staff.log");
        let stamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{stamp} {line}");
        }
    }
    for p in game.native_server_players() {
        if p.uuid != op && p.permission >= smp_invsee::LEVEL {
            game.native_tell(p.uuid, Text::literal(format!("[Staff] {line}")).with_color(text::GRAY));
        }
    }
}

/// Each tick: one load while nothing is open. Menus to open now; once a
/// second, every open menu read again, and those of operators gone closed.
pub(in crate::game) fn tick(game: &mut Game<'_>) {
    if !ACTIVE.load(Relaxed) {
        return;
    }
    let n = TICKS.fetch_add(1, Relaxed);
    let mut guard = state();
    let s = guard.as_mut().expect("made");
    let mut tell = Vec::new();
    for (op, target, name, kind) in std::mem::take(&mut s.pending) {
        let opened = s.invsee.open(op, target, &name, kind, &GameWorld(game));
        match opened {
            Ok(()) => {
                let window = game.native_open_menu(op, smp_invsee::ROWS, title(&name, kind), items(&s.invsee, op));
                match window {
                    Some(w) => s.invsee.views.get_mut(&op).expect("opened").window = Some(w),
                    None => {
                        s.invsee.views.remove(&op);
                    }
                }
            }
            Err(e) => tell.push((op, format!("Can't open {name}'s inventory: {e}."))),
        }
    }
    let mut summaries = Vec::new();
    if n % 20 == 0 {
        let on: Vec<(u128, String)> = game.native_server_players().into_iter().map(|p| (p.uuid, p.name)).collect();
        let ops: Vec<u128> = s.invsee.views.keys().copied().collect();
        for op in ops {
            match on.iter().find(|p| p.0 == op) {
                None => summaries.extend(s.invsee.close(op, &smp_store::uuid_string(op)).map(|l| (op, l))),
                Some(_) => {
                    if s.invsee.refresh(op, &GameWorld(game)) {
                        game.native_set_menu_items(op, items(&s.invsee, op));
                    }
                }
            }
        }
    }
    ACTIVE.store(!s.invsee.views.is_empty() || !s.pending.is_empty(), Relaxed);
    drop(guard);
    for (op, line) in tell {
        game.native_tell(op, Text::literal(line).with_color(text::RED));
    }
    for (op, line) in summaries {
        staff(game, op, &line);
    }
}

/// A click in an `/invsee` menu: true when it was one.
pub(in crate::game) fn menu_clicked(game: &mut Game<'_>, uuid: u128, window: i32, slot: i32, mode: i32) -> bool {
    let mut guard = state();
    let s = guard.as_mut().expect("made");
    if s.invsee.views.get(&uuid).is_none_or(|v| v.window != Some(window)) {
        return false;
    }
    let Some(dir) = game.native_world_dir() else {
        return true;
    };
    if !matches!(mode, 0 | 1) {
        return true;
    }
    // Turned off since it opened: closed, nothing moves.
    if !smp_glue::part_on(game, smp_switch::Part::Invsee) {
        drop(guard);
        game.native_close_menu(uuid);
        return true;
    }
    let mut asked: Option<Move> = None;
    // Each move is in the audit log before it is made; one the log
    // refuses isn't made.
    let mut record = |m: &Move| {
        audit(&dir, m, "asked")?;
        asked = Some(m.clone());
        Ok(())
    };
    let own = native_menu_inventory_slot(smp_invsee::SIZE, slot);
    let mut world = GameWorld(game);
    let result = match own {
        Some(i) => match world.0.native_player_stacks(uuid).and_then(|st| st.get(i).cloned().flatten()) {
            Some(stack) => s.invsee.click_own(uuid, i, &stack, &mut world, &mut record).map(|_| false),
            None => Ok(false),
        },
        None => match usize::try_from(slot) {
            Ok(m) if m < smp_invsee::SIZE => s.invsee.click_menu(uuid, m, &mut world, &mut record).map(|c| c == Some(true)),
            _ => Ok(false),
        },
    };
    if let (Some(m), Err(e)) = (&asked, &result)
        && let Err(err) = audit(&dir, m, &format!("refused: {e}"))
    {
        log::error!("[smp] invsee audit: {err}");
    }
    let name = game.native_server_players().into_iter().find(|p| p.uuid == uuid).map(|p| p.name);
    let close = matches!(result, Ok(true));
    let summary = if close { s.invsee.close(uuid, name.as_deref().unwrap_or("?")) } else { None };
    let refreshed = (!close).then(|| items(&s.invsee, uuid));
    ACTIVE.store(!s.invsee.views.is_empty() || !s.pending.is_empty(), Relaxed);
    drop(guard);
    if let Err(e) = &result {
        game.native_tell(uuid, Text::literal(format!("Nothing moved: {e}.")).with_color(text::RED));
    }
    if close {
        game.native_close_menu(uuid);
    } else if let Some(items) = refreshed {
        game.native_set_menu_items(uuid, items);
    }
    if let Some(line) = summary {
        staff(game, uuid, &line);
    }
    true
}

/// An `/invsee` menu closed: true when it was one.
pub(in crate::game) fn menu_closed(game: &mut Game<'_>, uuid: u128, window: i32) -> bool {
    let mut guard = state();
    let s = guard.as_mut().expect("made");
    if s.invsee.views.get(&uuid).is_none_or(|v| v.window != Some(window)) {
        return false;
    }
    let name = game.native_server_players().into_iter().find(|p| p.uuid == uuid).map(|p| p.name);
    let summary = s.invsee.close(uuid, name.as_deref().unwrap_or("?"));
    ACTIVE.store(!s.invsee.views.is_empty() || !s.pending.is_empty(), Relaxed);
    drop(guard);
    if let Some(line) = summary {
        staff(game, uuid, &line);
    }
    true
}
