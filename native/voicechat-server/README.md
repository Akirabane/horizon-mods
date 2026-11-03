# Voice chat server (Horizon native mod)

The server half of proximity voice chat on a Horizon dedicated server. Players who have a voice chat client mod
(azetech's Complex Voice Chat, any client that speaks Simple Voice Chat 2.6.x's protocol over Horizon's mod
messages) talk to the players near them, or to their group from anywhere. Players without one join and play as on
any server: the server sends them nothing.

- **The handshake** goes over Horizon's mod messages (`native_net_from_client` / `native_net_send_to_player`, the
  `horizon:mod_net` channel) on `voicechat:` channel names, the same messages and layouts as Simple Voice Chat 2.6.x
  (compatibility version 20). A client gets a fresh 16-byte secret when it asks; nothing is sent to anyone before.
- **Voices** go over UDP, through the server, never from player to player: no player learns another's address.
  Each player's packets are sealed with their own secret (AES-128-GCM). The server relays a voice to the players
  in the same dimension within `distance` blocks (`whisper_distance` for a whisper), and to the speaker's group
  wherever they are. Groups: normal (members aren't heard outside but hear who is near), open (heard and hearing
  nearby), isolated (members only).
- **Limits:** 75 voice packets a second a player (burst 30), Opus frames up to 1275 bytes, 128 groups, names and
  passwords up to 24 characters, group and state changes 2 a second a player (burst 10), one secret a second a
  player (more requests meanwhile get one answer), 24 messages a player a tick and at most 512 waiting.
- **Replays:** a packet that doesn't open with the player's key is dropped without an answer. A secret belongs to
  the first address that authenticates with it: a handshake copied and sent from elsewhere moves nothing. Exact
  copies are dropped too (the last 64 control packets' nonces, and voice packets' sequence numbers). A client the
  server stops hearing for 10 keep-alives (its NAT rebound, say) gets a fresh secret over the game connection and
  comes back from its new address with it; the old key then opens nothing.
- **No port is opened** until the first voice chat client asks. Then the relay binds `bind:port` (UDP).

## Config

`config/voicechat-server.json`, beside `server.properties`, written with its defaults the first time a client asks:

| Key | Default | |
|---|---|---|
| `enabled` | `true` | `false`: no port, no answer to clients |
| `port` | `24454` | the UDP port. A host that gives one port for the game usually forwards UDP on it too: use the game's port |
| `bind` | `0.0.0.0` | |
| `voice_host` | `""` | where clients send UDP if not the game server's address: a port, a host, or host:port |
| `distance` | `48.0` | blocks a voice carries |
| `whisper_distance` | `24.0` | |
| `groups` | `true` | players may make and join groups |
| `keep_alive_ms` | `1000` | a client silent 10 times this long shows as disconnected |
| `mtu` | `1024` | |

## Files

| File | What it is |
|---|---|
| `mixins/voicechat_server/horizon-mixin.toml` | The bundle: the relay crate, the glue module and four HEAD hooks (mod messages, server tick, player logged out, world closing) |
| `mixins/voicechat_server/relay/` | The relay crate (`voicechat_server_relay`): the wire format (`proto.rs`), the handshake, groups and UDP relay (`lib.rs`) and its tests over real UDP sockets (`tests.rs`). Depends on `ring` (AES-GCM) and `serde`, both in Horizon's `Cargo.lock` |
| `mixins/voicechat_server/glue.rs` | Added to the game as `game::voicechat_server`: the config, starting the relay, the players each tick |

It needs horizon-mixins 0.6.0 (bundle crates) and hook points API 57. Tests: `cargo test` here.

Not made with or endorsed by Simple Voice Chat or its author: it speaks the same protocol, written from its packet
layouts, with none of its code.
