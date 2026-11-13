// An explosion's blocks: claimed land is spared, and so are the animals
// and frames there it would reach.
fn smp_explosion_detonate(
    game: &crate::game::Game<'_>,
    dimension: &str,
    center: [f64; 3],
    blocks: &mut Vec<[i32; 3]>,
) {
    crate::game::smp_glue::explosion(game, dimension, blocks);
    crate::game::smp_claims_glue::blast(dimension, center);
}
