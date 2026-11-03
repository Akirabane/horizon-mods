// The end of each server tick: the relay learns where everyone is, and its
// messages leave for the players' clients.
fn voicechat_server_tick(
    game: &mut crate::game::Game<'_>,
    phase: crate::game::native_lifecycle::ServerTickPhase,
) {
    if matches!(
        phase,
        crate::game::native_lifecycle::ServerTickPhase::ServerTickEnd { .. }
    ) {
        crate::game::voicechat_server::tick(game);
    }
}
