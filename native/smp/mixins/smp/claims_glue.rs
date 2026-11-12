// The claims' hooks past breaking and clicking blocks (glue.rs `guard`):
// what is placed against a claim from outside, a claim's animals, frames and
// stands used by others, dispensers and lava outside it, projectiles, and
// the frames and animals a blast reaches in it. Rules in smp_claims.rs.
// Added to the game as `game::smp_claims_glue`.

use crate::game::{Game, smp_claims::Source, smp_glue};
use horizon_client::items::ItemStack;
use std::collections::HashMap;
use std::sync::Mutex;

fn block_of(p: [f64; 3]) -> [i32; 3] {
    p.map(|v| v.floor() as i32)
}

fn same_chunk(a: [i32; 3], b: [i32; 3]) -> bool {
    (a[0] >> 4, a[2] >> 4) == (b[0] >> 4, b[2] >> 4)
}

/// Whether using `stack` on a block puts something in the block beside it
/// (the face's side): a block, fire, a mob, an entity to place.
fn places_beside(game: &Game<'_>, stack: &ItemStack) -> bool {
    let id = &*stack.item;
    game.items()
        .get(&stack.item)
        .is_some_and(|i| i.block.is_some())
        || id.ends_with("_spawn_egg")
        || id.ends_with("_boat")
        || id.ends_with("_raft")
        || id.ends_with("minecart")
        || id.ends_with("_bucket")
        || matches!(
            id,
            "minecraft:flint_and_steel"
                | "minecraft:fire_charge"
                | "minecraft:armor_stand"
                | "minecraft:item_frame"
                | "minecraft:glow_item_frame"
                | "minecraft:painting"
                | "minecraft:end_crystal"
                | "minecraft:bone_meal"
        )
}

/// A player uses `stack` on the block at `pos`, on its side `face`: kept
/// when that puts something in another's claim from outside it (a block or
/// fire placed against the border; the block clicked itself is
/// `native_right_click_block`'s) (`native_use_item_on`).
pub(in crate::game) fn use_item_on(
    game: &mut Game<'_>,
    stack: &ItemStack,
    pos: [i32; 3],
    face: [i32; 3],
    claimed: &mut bool,
) {
    let beside = [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]];
    if *claimed || same_chunk(pos, beside) || !places_beside(game, stack) {
        return;
    }
    smp_glue::guard(game, beside, claimed);
}

/// A player uses an entity: a claim's animals, frames, stands and vehicles
/// keep to its members (no lead, saddle, ride, shears, dye, bucket, item
/// taken or swapped). Traders still trade (`native_interact_entity`).
pub(in crate::game) fn interact_entity(
    game: &mut Game<'_>,
    entity: i32,
    kind: &str,
    claimed: &mut bool,
) {
    if *claimed || matches!(kind, "minecraft:villager" | "minecraft:wandering_trader") {
        return;
    }
    if smp_glue::with_claims(|c| c.protects(kind)) != Some(true) {
        return;
    }
    let at = u32::try_from(entity)
        .ok()
        .and_then(|id| game.native_entity(id))
        .and_then(|e| game.native_entity_position(e));
    if let Some(at) = at {
        smp_glue::guard(game, block_of(at), claimed);
    }
}

/// A dispenser at `pos` facing `front` is about to act on the block in
/// front: it fails (as an empty one clicks) when that block is in a claim
/// of someone other than the dispenser's land's owner: no water, lava,
/// fire, TNT or shulker box put in from outside (`native_dispense_item`).
pub(in crate::game) fn dispense(
    dimension: &str,
    pos: [i32; 3],
    front: [i32; 3],
    stack: &ItemStack,
    answer: &mut Option<horizon_client::mod_api::NativeDispensed>,
) {
    let to = [pos[0] + front[0], pos[1] + front[1], pos[2] + front[2]];
    if answer.is_some() || same_chunk(pos, to) {
        return;
    }
    if !smp_glue::world_allows(dimension, pos, &[to]) {
        *answer = Some((Some(stack.clone()), 1001));
    }
}

/// Lava is about to set fire at `pos`: not in a claim near land of another
/// owner (`native_block_ignite`, cause `lava`; fire spreading is
/// `native_block_spread`).
pub(in crate::game) fn ignite(dimension: &str, pos: [i32; 3], cause: &str, cancel: &mut bool) {
    if *cancel || cause != "lava" {
        return;
    }
    if smp_glue::with_claims(|c| !c.lava_may_ignite(dimension, pos)) == Some(true) {
        *cancel = true;
    }
}

/// Projectiles launched by nobody (dispensers): the block each left from and
/// the tick, for `impact`. Kept a minute.
static LAUNCHED: Mutex<Option<HashMap<u32, ([i32; 3], u64)>>> = Mutex::new(None);

fn projectile(kind: &str) -> bool {
    matches!(
        kind.trim_start_matches("minecraft:"),
        "arrow"
            | "spectral_arrow"
            | "trident"
            | "snowball"
            | "egg"
            | "ender_pearl"
            | "splash_potion"
            | "lingering_potion"
            | "potion"
            | "experience_bottle"
            | "small_fireball"
            | "fireball"
            | "dragon_fireball"
            | "wither_skull"
            | "wind_charge"
            | "breeze_wind_charge"
            | "llama_spit"
            | "shulker_bullet"
    )
}

/// An entity joins the level: where a projectile left from
/// (`native_entity_join_level`). A string match for any other entity.
pub(in crate::game) fn joined(entity: u32, kind: &str, pos: [f64; 3], loaded: bool) {
    if loaded || !projectile(kind) {
        return;
    }
    let now = smp_glue::ticks();
    let mut map = LAUNCHED.lock().unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(HashMap::new);
    if map.len() >= 1024 {
        map.retain(|_, (_, t)| now.saturating_sub(*t) < 1200);
    }
    map.insert(entity, (block_of(pos), now));
}

/// Whether the SMP hears projectiles' impacts: when it is on in this world
/// (`native_hears_projectile_impacts`).
pub(in crate::game) fn hears(game: &Game<'_>, hears: &mut bool) {
    *hears |= smp_glue::claims_live(game);
}

/// A projectile hits a block or an entity (`native_projectile_impact`): in
/// another's claim, only a member's shot does anything (a mob's hit on an
/// entity too, and nobody's from inside the claim): no button pressed,
/// target lit, chorus or pot broken, TNT lit, crystal set off, pearl landed,
/// potion burst, animal, frame or stand hit. A shot refused is spent: taken
/// out of the world, so it doesn't fly on through the claim.
#[allow(clippy::too_many_arguments)]
pub(in crate::game) fn impact(
    game: &mut Game<'_>,
    dimension: &str,
    projectile: u32,
    owner: u32,
    entity: Option<u32>,
    entity_kind: &str,
    block: [i32; 3],
    at: [f64; 3],
    canceled: &mut bool,
) {
    if *canceled {
        return;
    }
    let target = if entity.is_some() {
        block_of(at)
    } else {
        block
    };
    if smp_glue::with_claims(|c| c.owner(dimension, target[0], target[2]).is_some()) != Some(true) {
        return;
    }
    let source = if owner == 0 {
        let launched = LAUNCHED.lock().unwrap_or_else(|e| e.into_inner());
        launched
            .as_ref()
            .and_then(|m| m.get(&projectile))
            .map_or(Source::Unknown, |(from, _)| Source::Launched(*from))
    } else {
        game.native_server_players()
            .into_iter()
            .find(|p| p.entity_id != 0 && p.entity_id as u32 == owner)
            .map_or(Source::Mob, |p| Source::Player(p.uuid, p.permission))
    };
    let hit = entity.map(|_| entity_kind);
    if smp_glue::with_claims(|c| c.impact_allowed(dimension, target, hit, source)) == Some(false) {
        *canceled = true;
        game.creatures.projectiles.retain(|p| p.id != projectile);
    }
}

/// This tick's blasts near a claim (dimension, centre).
struct Blasts {
    tick: u64,
    centres: Vec<(String, [f64; 3])>,
}
static BLASTS: Mutex<Blasts> = Mutex::new(Blasts {
    tick: u64::MAX,
    centres: Vec::new(),
});
/// How far a blast reaches entities: twice the strongest vanilla power (a
/// charged creeper 6, a wither's spawn 7), plus its box.
const REACH: f64 = 16.0;

/// A blast at `center` is about to hit (`native_explosion_detonate`): when
/// it is near a claim, it is noted for `hanging_break`. Blocks are spared by
/// `smp_glue::explosion`, animals by `spare`. Nothing more for a blast far
/// from every claim.
pub(in crate::game) fn blast(dimension: &str, center: [f64; 3]) {
    let c = block_of(center);
    if smp_glue::with_claims(|cl| cl.near(dimension, c, REACH as i32)) != Some(true) {
        return;
    }
    let now = smp_glue::ticks();
    let mut b = BLASTS.lock().unwrap_or_else(|e| e.into_inner());
    if b.tick != now {
        b.tick = now;
        b.centres.clear();
    }
    b.centres.push((dimension.to_owned(), center));
}

/// A blast is about to hurt and push `entities` (`native_explosion_entities`):
/// the protected mobs on claimed land are spared.
pub(in crate::game) fn spare(game: &Game<'_>, dimension: &str, entities: &mut Vec<u32>) {
    smp_glue::with_claims(|cl| {
        entities.retain(|&id| {
            !game.mobs.iter().find(|m| m.id == id).is_some_and(|m| {
                let p = block_of([m.position.x, m.position.y, m.position.z]);
                cl.owner(dimension, p[0], p[2]).is_some()
                    && cl.protects(&crate::game::native_hooks::mob_kind(m))
            })
        });
    });
}

/// A painting, frame or other hanging entity at `pos` is about to break
/// (`native_hanging_break`): a player's break is theirs to make only on
/// land they may build on; one by a blast this tick near it spares claimed
/// land. A break because its wall went (none of these) goes on.
pub(in crate::game) fn hanging_break(
    game: &mut Game<'_>,
    player: Option<u128>,
    pos: [i32; 3],
    cancel: &mut bool,
) {
    if *cancel {
        return;
    }
    if player.is_some() {
        smp_glue::guard(game, pos, cancel);
        return;
    }
    let dimension = game.dim.id();
    let blasted = {
        let b = BLASTS.lock().unwrap_or_else(|e| e.into_inner());
        b.tick == smp_glue::ticks()
            && b.centres.iter().any(|(d, c)| {
                let q = [
                    f64::from(pos[0]) + 0.5 - c[0],
                    f64::from(pos[1]) + 0.5 - c[1],
                    f64::from(pos[2]) + 0.5 - c[2],
                ];
                d == dimension && q[0] * q[0] + q[1] * q[1] + q[2] * q[2] <= REACH * REACH
            })
    };
    if blasted
        && smp_glue::with_claims(|c| c.owner(dimension, pos[0], pos[2]).is_some()) == Some(true)
    {
        *cancel = true;
    }
}
