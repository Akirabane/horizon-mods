//! The server half of proximity voice chat on Horizon: it answers the voice
//! chat clients' handshake (sent over Horizon's mod messages) and relays their
//! voices over UDP to the players who should hear them, by distance and
//! group. Players without a voice chat client never hear of it: nothing is
//! sent to a player until their client asks.
//!
//! Voices go through the server, never player to player, so nobody learns
//! another player's address. Each player's packets are sealed with their own
//! key (AES-128-GCM), which they get over the game connection.
pub mod proto;

use proto::{FromClient, GroupKind, PlayerState, Request, ToClient};
use ring::rand::{SecureRandom, SystemRandom};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// `config/voicechat-server.json`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    /// Off: no port is opened and clients get no answer.
    pub enabled: bool,
    /// The UDP port voices use. A host that gives one port for the game
    /// (TCP) usually forwards UDP on it too: use the game's port then.
    pub port: u16,
    /// The address the UDP socket binds.
    pub bind: String,
    /// Where clients send UDP, when not the game server's own address:
    /// a port, a host, or host:port.
    pub voice_host: String,
    /// How far a voice carries, in blocks.
    pub distance: f64,
    /// How far a whisper carries.
    pub whisper_distance: f64,
    /// Players may make and join groups.
    pub groups: bool,
    pub keep_alive_ms: u32,
    pub mtu: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 24454,
            bind: "0.0.0.0".into(),
            voice_host: String::new(),
            distance: 48.0,
            whisper_distance: 24.0,
            groups: true,
            keep_alive_ms: 1000,
            mtu: 1024,
        }
    }
}

/// A player on the game server, as the last tick saw them.
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub uuid: u128,
    pub name: String,
    pub dimension: String,
    pub position: [f64; 3],
}

/// A message for a player's client: (player, channel, data).
pub type Message = (u128, String, Vec<u8>);

/// Groups at most, and voice packets a second a player may send.
const MAX_GROUPS: usize = 128;
const MIC_PER_SECOND: f64 = 75.0;
const MIC_BURST: f64 = 30.0;
/// Group and voice state changes a second a player may make.
const OPS_PER_SECOND: f64 = 2.0;
const OPS_BURST: f64 = 10.0;
/// Secrets a player's own requests get at most this often; more requests
/// meanwhile are answered once, when the time is up.
const SECRET_EVERY: Duration = Duration::from_secs(1);
/// Messages waiting for one player at most; more are dropped.
const MAX_QUEUED: usize = 512;
/// The nonces of a player's last control packets remembered, to drop exact
/// replays; and how far behind the newest a voice packet's sequence may be.
const SEEN_NONCES: usize = 64;
const MIC_WINDOW: i128 = 64;

/// A player's voice connection, for the life of one secret.
///
/// The first address that authenticates holds the secret: no other address
/// can use it, so a handshake captured and sent again from elsewhere moves
/// nothing. A client that moves (a NAT that rebinds) gets a fresh secret
/// over the game connection, which the game session authenticates: when the
/// server stops hearing it, or when it asks again. The old key then opens
/// nothing.
struct Client {
    secret: [u8; 16],
    cipher: proto::Cipher,
    addr: Option<SocketAddr>,
    /// Answered the connection check: its voice connection works.
    connected: bool,
    last_keep_alive: Instant,
    disabled: bool,
    group: Option<u128>,
    tokens: f64,
    refilled: Instant,
    ops: f64,
    ops_refilled: Instant,
    /// When this secret was given, and whether a request for another waits.
    issued: Instant,
    asked_again: bool,
    /// Nonces of the control packets already taken (the newest last).
    seen: VecDeque<[u8; 12]>,
    /// The newest voice sequence, and which of the 64 before it arrived.
    mic_top: Option<i64>,
    mic_seen: u64,
}

impl Client {
    fn new(secret: [u8; 16], now: Instant) -> Self {
        Self {
            secret,
            cipher: proto::Cipher::new(&secret),
            addr: None,
            connected: false,
            last_keep_alive: now,
            disabled: false,
            group: None,
            tokens: MIC_BURST,
            refilled: now,
            ops: OPS_BURST,
            ops_refilled: now,
            issued: now,
            asked_again: false,
            seen: VecDeque::new(),
            mic_top: None,
            mic_seen: 0,
        }
    }

    /// A control packet's nonce: false if it was already taken.
    fn fresh_nonce(&mut self, nonce: [u8; 12]) -> bool {
        if self.seen.contains(&nonce) {
            return false;
        }
        if self.seen.len() >= SEEN_NONCES {
            self.seen.pop_front();
        }
        self.seen.push_back(nonce);
        true
    }

    /// A voice packet's sequence: false if it was already relayed or is too
    /// old (a replay). A client counts up from 0 for each secret.
    fn fresh_sequence(&mut self, sequence: i64) -> bool {
        let Some(top) = self.mic_top else {
            self.mic_top = Some(sequence);
            self.mic_seen = 1;
            return true;
        };
        let ahead = i128::from(sequence) - i128::from(top);
        if ahead > 0 {
            self.mic_seen = if ahead >= MIC_WINDOW {
                1
            } else {
                (self.mic_seen << ahead) | 1
            };
            self.mic_top = Some(sequence);
            return true;
        }
        let back = -ahead;
        if back >= MIC_WINDOW {
            return false;
        }
        let bit = 1u64 << back;
        if self.mic_seen & bit != 0 {
            return false;
        }
        self.mic_seen |= bit;
        true
    }

    /// One group or state change, if the player has one left.
    fn spend_op(&mut self, now: Instant) -> bool {
        self.ops = (self.ops
            + now.duration_since(self.ops_refilled).as_secs_f64() * OPS_PER_SECOND)
            .min(OPS_BURST);
        self.ops_refilled = now;
        if self.ops < 1.0 {
            return false;
        }
        self.ops -= 1.0;
        true
    }
}

struct Group {
    name: String,
    password: Option<String>,
    kind: GroupKind,
}

struct State {
    config: Config,
    /// The UDP port the socket got.
    port: u16,
    clients: HashMap<u128, Client>,
    players: HashMap<u128, Player>,
    groups: HashMap<u128, Group>,
    /// Players whose state changed since the last tick, to tell everyone.
    changed: HashSet<u128>,
    outbox: HashMap<u128, VecDeque<(String, Vec<u8>)>>,
}

struct Shared {
    state: Mutex<State>,
    socket: UdpSocket,
    rng: SystemRandom,
    running: AtomicBool,
}

/// The running voice server: its UDP socket and thread. Dropping it stops them.
pub struct Server {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Server {
    /// Opens the UDP port and starts relaying.
    pub fn start(config: Config) -> io::Result<Self> {
        if !config.enabled {
            return Err(io::Error::other("voice chat is turned off in its config"));
        }
        let socket = UdpSocket::bind((config.bind.as_str(), config.port))?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        let port = socket.local_addr()?.port();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                config,
                port,
                clients: HashMap::new(),
                players: HashMap::new(),
                groups: HashMap::new(),
                changed: HashSet::new(),
                outbox: HashMap::new(),
            }),
            socket,
            rng: SystemRandom::new(),
            running: AtomicBool::new(true),
        });
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("voicechat-relay".into())
            .spawn(move || relay(&worker))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// The UDP address voices arrive on.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.shared.socket.local_addr()
    }

    /// Each server tick: the players on the server now. Voice chat forgets
    /// the ones who left, and tells everyone what changed.
    pub fn tick(&self, players: Vec<Player>) {
        let mut st = lock(&self.shared.state);
        st.players = players.into_iter().map(|p| (p.uuid, p)).collect();
        let gone: Vec<u128> = st
            .clients
            .keys()
            .filter(|u| !st.players.contains_key(u))
            .copied()
            .collect();
        for uuid in gone {
            st.forget(uuid);
        }
        // Requests that came too soon after a secret get one now.
        let due: Vec<u128> = st
            .clients
            .iter()
            .filter(|(_, c)| c.asked_again && c.issued.elapsed() >= SECRET_EVERY)
            .map(|(u, _)| *u)
            .collect();
        for uuid in due {
            st.give_secret(&self.shared.rng, uuid, false);
        }
        let changed: Vec<u128> = st.changed.drain().collect();
        for uuid in changed {
            if let Some(s) = st.player_state(uuid) {
                st.tell_all(proto::STATE, proto::state(&s));
            }
        }
    }

    /// Whether `player` was on the server at the last tick.
    pub fn knows(&self, player: u128) -> bool {
        lock(&self.shared.state).players.contains_key(&player)
    }

    /// A player left the game.
    pub fn left(&self, player: u128) {
        lock(&self.shared.state).forget(player);
    }

    /// A message from `player`'s client on a `voicechat:` channel.
    pub fn message(&self, player: u128, channel: &str, data: &[u8]) {
        let Some(Ok(request)) = Request::read(channel, data) else {
            return;
        };
        let mut st = lock(&self.shared.state);
        if !st.players.contains_key(&player) {
            return;
        }
        match request {
            Request::Secret { version } => {
                if version != proto::COMPATIBILITY_VERSION {
                    return;
                }
                if let Some(c) = st.clients.get_mut(&player)
                    && c.issued.elapsed() < SECRET_EVERY
                {
                    c.asked_again = true;
                    return;
                }
                st.give_secret(&self.shared.rng, player, false);
            }
            Request::State { disabled } => {
                if let Some(c) = st.clients.get_mut(&player)
                    && c.spend_op(Instant::now())
                    && c.disabled != disabled
                {
                    c.disabled = disabled;
                    st.changed.insert(player);
                }
            }
            Request::CreateGroup {
                name,
                password,
                kind,
            } => {
                let name = name.trim().to_owned();
                if !st.config.groups
                    || name.is_empty()
                    || st.groups.len() >= MAX_GROUPS
                    || !st.spend_op(player)
                {
                    return;
                }
                let mut id = [0u8; 16];
                if self.shared.rng.fill(&mut id).is_err() {
                    return;
                }
                let id = u128::from_be_bytes(id);
                let password = password
                    .map(|p| p.trim().to_owned())
                    .filter(|p| !p.is_empty());
                let add = proto::add_group(id, &name, password.is_some(), kind);
                st.groups.insert(
                    id,
                    Group {
                        name,
                        password,
                        kind,
                    },
                );
                st.tell_all(proto::ADD_GROUP, add);
                st.join(player, id);
            }
            Request::JoinGroup { group, password } => {
                if !st.config.groups || !st.groups.contains_key(&group) || !st.spend_op(player) {
                    return;
                }
                let Some(g) = st.groups.get(&group) else {
                    return;
                };
                let ok = g.password.is_none()
                    || g.password.as_deref() == password.as_deref().map(str::trim);
                if ok {
                    st.join(player, group);
                } else {
                    let current = st.clients.get(&player).and_then(|c| c.group);
                    st.tell(
                        player,
                        proto::JOINED_GROUP,
                        proto::joined_group(current, true),
                    );
                }
            }
            Request::LeaveGroup => {
                let Some(c) = st.clients.get_mut(&player) else {
                    return;
                };
                if c.group.is_none() || !c.spend_op(Instant::now()) {
                    return;
                }
                let Some(g) = c.group.take() else { return };
                st.tell(
                    player,
                    proto::JOINED_GROUP,
                    proto::joined_group(None, false),
                );
                st.changed.insert(player);
                st.leave_group_cleanup(g);
            }
        }
    }

    /// What to send to the players' clients now, at most `per_player` messages each (the rest wait for the
    /// next call).
    pub fn outgoing(&self, per_player: usize) -> Vec<Message> {
        let mut st = lock(&self.shared.state);
        let mut out = Vec::new();
        st.outbox.retain(|player, queue| {
            for _ in 0..per_player {
                let Some((channel, data)) = queue.pop_front() else {
                    break;
                };
                out.push((*player, channel, data));
            }
            !queue.is_empty()
        });
        out
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl State {
    /// Queues a message for `player`'s client (dropped when MAX_QUEUED wait).
    fn tell(&mut self, player: u128, channel: &str, data: Vec<u8>) {
        let queue = self.outbox.entry(player).or_default();
        if queue.len() < MAX_QUEUED {
            queue.push_back((channel.to_owned(), data));
        }
    }

    fn spend_op(&mut self, player: u128) -> bool {
        self.clients
            .get_mut(&player)
            .is_some_and(|c| c.spend_op(Instant::now()))
    }

    /// A fresh secret for `player`: the old one opens nothing any more.
    /// `rekey`: the server's own, for a client it stopped hearing (it keeps its
    /// group and state, and gets only the secret). Otherwise the client asked:
    /// it starts over and gets everyone's state and the groups, instead of
    /// whatever still waited for it.
    fn give_secret(&mut self, rng: &SystemRandom, player: u128, rekey: bool) {
        let mut secret = [0u8; 16];
        if rng.fill(&mut secret).is_err() {
            return;
        }
        let mut client = Client::new(secret, Instant::now());
        let old = self.clients.remove(&player);
        if rekey && let Some(old) = &old {
            client.group = old.group;
            client.disabled = old.disabled;
            client.ops = old.ops;
            client.ops_refilled = old.ops_refilled;
        }
        self.clients.insert(player, client);
        if !rekey && let Some(g) = old.and_then(|o| o.group) {
            self.leave_group_cleanup(g);
        }
        let c = &self.config;
        let body = proto::secret(&proto::SecretInfo {
            secret: &secret,
            port: self.port,
            player,
            mtu: c.mtu.min(i32::MAX as u32) as i32,
            distance: c.distance,
            keep_alive_ms: c.keep_alive_ms.clamp(100, 60_000) as i32,
            groups: c.groups,
            voice_host: &c.voice_host,
        });
        self.changed.insert(player);
        if rekey {
            // Past the cap: the secret is what gets the client back.
            self.outbox
                .entry(player)
                .or_default()
                .push_back((proto::SECRET.to_owned(), body));
            return;
        }
        self.outbox.remove(&player);
        self.tell(player, proto::SECRET, body);
        let all: Vec<PlayerState> = self
            .clients
            .keys()
            .filter_map(|u| self.player_state(*u))
            .collect();
        self.tell(player, proto::STATES, proto::states(&all));
        let groups: Vec<Vec<u8>> = self
            .groups
            .iter()
            .map(|(id, g)| proto::add_group(*id, &g.name, g.password.is_some(), g.kind))
            .collect();
        for g in groups {
            self.tell(player, proto::ADD_GROUP, g);
        }
    }

    /// To every player with a voice chat client.
    fn tell_all(&mut self, channel: &str, data: Vec<u8>) {
        let all: Vec<u128> = self.clients.keys().copied().collect();
        for p in all {
            self.tell(p, channel, data.clone());
        }
    }

    fn player_state(&self, uuid: u128) -> Option<PlayerState> {
        let c = self.clients.get(&uuid)?;
        Some(PlayerState {
            uuid,
            name: self
                .players
                .get(&uuid)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            disabled: c.disabled,
            disconnected: !c.connected,
            group: c.group,
        })
    }

    fn join(&mut self, player: u128, group: u128) {
        let Some(c) = self.clients.get_mut(&player) else {
            return;
        };
        let old = c.group.replace(group);
        self.tell(
            player,
            proto::JOINED_GROUP,
            proto::joined_group(Some(group), false),
        );
        self.changed.insert(player);
        if let Some(old) = old
            && old != group
        {
            self.leave_group_cleanup(old);
        }
    }

    /// An empty group goes away.
    fn leave_group_cleanup(&mut self, group: u128) {
        if self.clients.values().any(|c| c.group == Some(group)) {
            return;
        }
        if self.groups.remove(&group).is_some() {
            self.tell_all(proto::REMOVE_GROUP, proto::remove_group(group));
        }
    }

    fn forget(&mut self, uuid: u128) {
        self.outbox.remove(&uuid);
        self.changed.remove(&uuid);
        let Some(c) = self.clients.remove(&uuid) else {
            return;
        };
        self.tell_all(proto::REMOVE_STATE, proto::remove_state(uuid));
        if let Some(g) = c.group {
            self.leave_group_cleanup(g);
        }
    }

    /// Who hears `sender`, and how: (listener, address, positional range or None for the group's voice).
    fn listeners(&self, sender: u128, whispering: bool) -> Vec<(u128, SocketAddr, Option<f64>)> {
        let Some(from) = self.clients.get(&sender) else {
            return Vec::new();
        };
        let from_kind = from.group.and_then(|g| self.groups.get(&g)).map(|g| g.kind);
        let at = self.players.get(&sender);
        let range = if whispering {
            self.config.whisper_distance
        } else {
            self.config.distance
        };
        let mut out = Vec::new();
        for (uuid, c) in &self.clients {
            if *uuid == sender || c.disabled || !c.connected {
                continue;
            }
            let Some(addr) = c.addr else { continue };
            if from.group.is_some() && from.group == c.group {
                out.push((*uuid, addr, None));
                continue;
            }
            let to_kind = c.group.and_then(|g| self.groups.get(&g)).map(|g| g.kind);
            // Normal and isolated groups aren't heard outside; isolated ones hear nobody outside.
            if matches!(from_kind, Some(GroupKind::Normal | GroupKind::Isolated))
                || to_kind == Some(GroupKind::Isolated)
            {
                continue;
            }
            let (Some(a), Some(b)) = (at, self.players.get(uuid)) else {
                continue;
            };
            if a.dimension != b.dimension {
                continue;
            }
            let d = (0..3)
                .map(|i| (a.position[i] - b.position[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            if d <= range {
                out.push((*uuid, addr, Some(range)));
            }
        }
        out
    }
}

/// The UDP side: handshakes, keep-alives and voices.
fn relay(shared: &Shared) {
    let mut buf = vec![0u8; 4096];
    let mut last_keep_alive = Instant::now();
    while shared.running.load(Ordering::Relaxed) {
        match shared.socket.recv_from(&mut buf) {
            Ok((n, from)) => datagram(shared, &buf[..n], from),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            // Never spin on an error that comes back at once.
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
        let every = Duration::from_millis(u64::from(
            lock(&shared.state).config.keep_alive_ms.clamp(100, 60_000),
        ));
        if last_keep_alive.elapsed() >= every {
            last_keep_alive = Instant::now();
            keep_alive(shared, every);
        }
    }
}

fn send(shared: &Shared, cipher: &proto::Cipher, to: SocketAddr, packet: &ToClient) {
    if let Some(sealed) = cipher.seal(&shared.rng, &packet.write()) {
        let _ = shared.socket.send_to(&proto::server_datagram(&sealed), to);
    }
}

/// Keep-alives to every authenticated client. One that stopped answering
/// gets a fresh secret over the game connection: if it moved (its NAT
/// rebound), it comes back from its new address with that secret.
fn keep_alive(shared: &Shared, every: Duration) {
    let mut st = lock(&shared.state);
    let mut lost = Vec::new();
    for (uuid, c) in st.clients.iter_mut() {
        let Some(addr) = c.addr else { continue };
        if c.connected && c.last_keep_alive.elapsed() > every * 10 {
            c.connected = false;
            lost.push(*uuid);
            continue;
        }
        send(shared, &c.cipher, addr, &ToClient::KeepAlive);
    }
    for uuid in lost {
        st.give_secret(&shared.rng, uuid, true);
    }
}

fn datagram(shared: &Shared, datagram: &[u8], from: SocketAddr) {
    let Ok((player, sealed)) = proto::client_datagram(datagram) else {
        return;
    };
    let mut st = lock(&shared.state);
    let Some(c) = st.clients.get_mut(&player) else {
        return;
    };
    let Some(plain) = c.cipher.open(sealed) else {
        return;
    };
    let Ok(packet) = FromClient::read(&plain) else {
        return;
    };
    // It opened, so it holds at least a nonce and a tag.
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&sealed[..12]);
    let now = Instant::now();
    if let FromClient::Authenticate {
        player: who,
        secret,
    } = packet
    {
        if who != player || !constant_eq(&secret, &c.secret) {
            return;
        }
        // The secret belongs to the first address that authenticated with it.
        if c.addr.is_some_and(|a| a != from) || !c.fresh_nonce(nonce) {
            return;
        }
        c.addr = Some(from);
        send(shared, &c.cipher, from, &ToClient::AuthenticateAck);
        return;
    }
    // Everything else only from the address that authenticated.
    if c.addr != Some(from) {
        return;
    }
    if let FromClient::Mic { sequence, .. } = packet {
        if !c.fresh_sequence(sequence) {
            return;
        }
    } else if !c.fresh_nonce(nonce) {
        return;
    }
    match packet {
        FromClient::ConnectionCheck => {
            c.last_keep_alive = now;
            if !c.connected {
                c.connected = true;
                st.changed.insert(player);
            }
            let c = &st.clients[&player];
            send(shared, &c.cipher, from, &ToClient::ConnectionCheckAck);
        }
        FromClient::KeepAlive => {
            c.last_keep_alive = now;
            if !c.connected {
                c.connected = true;
                st.changed.insert(player);
            }
        }
        // The server never pings, so this isn't an answer: not echoed (a
        // client that echoes pings would otherwise bounce it forever).
        FromClient::Ping { .. } => {}
        FromClient::Mic {
            data,
            sequence,
            whispering,
        } => {
            if c.disabled || !c.connected {
                return;
            }
            c.tokens = (c.tokens + now.duration_since(c.refilled).as_secs_f64() * MIC_PER_SECOND)
                .min(MIC_BURST);
            c.refilled = now;
            if c.tokens < 1.0 {
                return;
            }
            c.tokens -= 1.0;
            for (listener, addr, range) in st.listeners(player, whispering) {
                let packet = match range {
                    Some(r) => ToClient::Positional {
                        sender: player,
                        data: &data,
                        sequence,
                        distance: r as f32,
                        whispering,
                    },
                    None => ToClient::Group {
                        sender: player,
                        data: &data,
                        sequence,
                    },
                };
                send(shared, &st.clients[&listener].cipher, addr, &packet);
            }
        }
        FromClient::Authenticate { .. } => {}
    }
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests;
