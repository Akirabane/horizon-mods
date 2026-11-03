// Voice chat's server half in the game (`game::voicechat_server`): the relay
// (crate voicechat_server_relay) opens its UDP port when the first voice chat
// client asks for a secret, hears the clients' messages through Horizon's mod
// messages, and learns each tick where every player is. A player without a
// voice chat client is never sent anything.

use crate::game::Game;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use voicechat_server_relay::{Config, Player, Server, proto};

/// Messages to one player a tick at most (mod messages allow 32, shared with other mods).
const PER_TICK: usize = 24;
/// The config file in the server's `config/` folder.
const FILE: &str = "voicechat-server.json";
/// After the port failed to open, wait this long before trying again.
const RETRY: Duration = Duration::from_secs(60);

enum Relay {
    Off,
    Running(Server),
    Failed(Instant),
}

static RELAY: Mutex<Relay> = Mutex::new(Relay::Off);

fn relay() -> std::sync::MutexGuard<'static, Relay> {
    RELAY.lock().unwrap_or_else(|e| e.into_inner())
}

fn players(game: &Game<'_>) -> Vec<Player> {
    game.native_server_players()
        .into_iter()
        .map(|p| Player {
            uuid: p.uuid,
            name: p.name,
            dimension: p.dimension,
            position: p.position,
        })
        .collect()
}

/// `config/voicechat-server.json`, written with its defaults the first time
/// and again with the keys a newer version adds (values kept). A file that
/// doesn't read is left alone and the defaults apply.
fn config() -> Config {
    let Ok(dir) = Game::native_config_dir() else {
        return Config::default();
    };
    let path = dir.join(FILE);
    let config = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Config>(&text) {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "Voice chat: {} doesn't read ({e}); using the defaults",
                    path.display()
                );
                return Config::default();
            }
        },
        Err(_) => Config::default(),
    };
    if let Ok(text) = serde_json::to_string_pretty(&config) {
        let _ = std::fs::write(&path, text + "\n");
    }
    config
}

/// A message from a player's client: only `voicechat:` channels are ours.
pub(super) fn message(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    if !channel.starts_with("voicechat:") {
        return;
    }
    let mut relay = relay();
    if !matches!(*relay, Relay::Running(_)) {
        if channel != proto::REQUEST_SECRET {
            return;
        }
        if let Relay::Failed(at) = *relay
            && at.elapsed() < RETRY
        {
            return;
        }
        let config = config();
        match Server::start(config.clone()) {
            Ok(server) => {
                let at = server
                    .local_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_default();
                log::info!("Voice chat: relaying voices on UDP {at}");
                *relay = Relay::Running(server);
            }
            Err(e) => {
                log::warn!(
                    "Voice chat: not started on UDP {}:{}: {e}",
                    config.bind,
                    config.port
                );
                *relay = Relay::Failed(Instant::now());
                return;
            }
        }
    }
    let Relay::Running(server) = &*relay else {
        return;
    };
    // A player who joined since the last tick (or a relay that just started):
    // learn who is on first. Not for every message: it lists every player.
    if !server.knows(player) {
        server.tick(players(game));
    }
    server.message(player, channel, data);
}

/// Each server tick, once the relay runs.
pub(super) fn tick(game: &mut Game<'_>) {
    let out = {
        let relay = relay();
        let Relay::Running(server) = &*relay else {
            return;
        };
        server.tick(players(game));
        server.outgoing(PER_TICK)
    };
    for (player, channel, data) in out {
        if let Err(e) = game.native_net_send_to_player(player, &channel, &data) {
            log::debug!("Voice chat: {channel} not sent: {e}");
        }
    }
}

pub(super) fn left(player: u128) {
    if let Relay::Running(server) = &*relay() {
        server.left(player);
    }
}

/// The world closed: the port closes too (it opens again on the next request).
pub(super) fn stop() {
    *relay() = Relay::Off;
}
