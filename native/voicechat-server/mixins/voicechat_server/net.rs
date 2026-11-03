// A voice chat client's message (`voicechat:` channels) to the relay.
fn voicechat_server_net(
    game: &mut crate::game::Game<'_>,
    player: u128,
    channel: &str,
    data: &[u8],
) {
    crate::game::voicechat_server::message(game, player, channel, data);
}
