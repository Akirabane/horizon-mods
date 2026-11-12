// The entities an explosion is about to hurt: the claim's animals are spared.
fn smp_explosion_entities(game: &crate::game::Game<'_>, dimension: &str, entities: &mut Vec<u32>) {
    crate::game::smp_claims_glue::spare(game, dimension, entities);
}
