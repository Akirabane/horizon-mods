// A player left: the relay forgets them and tells the others.
fn voicechat_server_logged_out(player: u128) {
    crate::game::voicechat_server::left(player);
}
