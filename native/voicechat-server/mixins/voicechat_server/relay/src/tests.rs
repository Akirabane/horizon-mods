//! The relay with real UDP sockets on localhost: clients that speak the
//! wire format the way a voice chat client does, written from the layouts.
use super::*;
use proto::{In, Out};

const A: u128 = 0xA;
const B: u128 = 0xB;
const FAR: u128 = 0xC;
const NETHER: u128 = 0xD;
const PLAIN: u128 = 0xE;

fn player(uuid: u128, name: &str, dimension: &str, x: f64) -> Player {
    Player {
        uuid,
        name: name.into(),
        dimension: dimension.into(),
        position: [x, 64.0, 0.0],
    }
}

fn world() -> Vec<Player> {
    vec![
        player(A, "alice", "minecraft:overworld", 0.0),
        player(B, "bob", "minecraft:overworld", 10.0),
        player(FAR, "carol", "minecraft:overworld", 500.0),
        player(NETHER, "dave", "minecraft:the_nether", 0.0),
        // Joined without a voice chat client: never asks, never told.
        player(PLAIN, "erin", "minecraft:overworld", 5.0),
    ]
}

fn server() -> Server {
    server_with(Config::default())
}

fn server_with(config: Config) -> Server {
    let s = Server::start(Config {
        bind: "127.0.0.1".into(),
        port: 0,
        ..config
    })
    .unwrap();
    s.tick(world());
    s
}

/// The messages for `who` among `all`.
fn for_player(all: &[Message], who: u128) -> Vec<(String, Vec<u8>)> {
    all.iter()
        .filter(|m| m.0 == who)
        .map(|m| (m.1.clone(), m.2.clone()))
        .collect()
}

struct Voice {
    uuid: u128,
    secret: [u8; 16],
    cipher: proto::Cipher,
    socket: UdpSocket,
    server: SocketAddr,
    rng: SystemRandom,
    /// The datagrams of its handshake: authenticate, connection check.
    handshake: Vec<Vec<u8>>,
}

impl Voice {
    /// Reads the secret the server gave, then authenticates and checks the connection.
    fn connect(uuid: u128, secret_message: &[u8]) -> Self {
        let mut r = In::new(secret_message);
        let secret: [u8; 16] = r.take(16).unwrap().try_into().unwrap();
        let port = r.int().unwrap() as u16;
        assert_eq!(r.uuid().unwrap(), uuid);
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let mut v = Self {
            uuid,
            secret,
            cipher: proto::Cipher::new(&secret),
            socket,
            server: SocketAddr::from(([127, 0, 0, 1], port)),
            rng: SystemRandom::new(),
            handshake: Vec::new(),
        };
        let auth = v.send(
            &Out::default()
                .byte(5)
                .uuid(uuid)
                .done()
                .into_iter()
                .chain(secret)
                .collect::<Vec<_>>(),
        );
        assert_eq!(v.next(|p| p == [6]), Some(vec![6]), "authenticated");
        let check = v.send(&[9]);
        assert_eq!(v.next(|p| p == [10]), Some(vec![10]), "connection checked");
        v.handshake = vec![auth, check];
        v
    }
    /// Sends a sealed packet; returns the datagram.
    fn send(&self, plain: &[u8]) -> Vec<u8> {
        let sealed = self.cipher.seal(&self.rng, plain).unwrap();
        let datagram = Out::default()
            .byte(proto::MAGIC)
            .uuid(self.uuid)
            .bytes(&sealed)
            .done();
        self.socket.send_to(&datagram, self.server).unwrap();
        datagram
    }
    /// Answers keep-alives for a while, as a live client does.
    fn idle(&self, for_ms: u64) {
        let until = Instant::now() + Duration::from_millis(for_ms);
        while Instant::now() < until {
            self.next(|_| false);
        }
    }
    fn talk(&self, sequence: i64, whispering: bool) -> Vec<u8> {
        self.send(
            &Out::default()
                .byte(1)
                .bytes(&[0xAB; 40])
                .long(sequence)
                .flag(whispering)
                .done(),
        )
    }
    /// The next packet `wanted` accepts, skipping keep-alives (answered) and others; None after a quiet while.
    fn next(&self, wanted: impl Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
        let mut buf = [0u8; 4096];
        let until = Instant::now() + Duration::from_millis(900);
        while Instant::now() < until {
            let Ok((n, _)) = self.socket.recv_from(&mut buf) else {
                continue;
            };
            let mut r = In::new(&buf[..n]);
            assert_eq!(r.byte().unwrap(), proto::MAGIC);
            let plain = self
                .cipher
                .open(r.bytes(4096).unwrap())
                .expect("sealed with my key");
            if plain == [8] {
                self.send(&[8]);
            }
            if wanted(&plain) {
                return Some(plain);
            }
        }
        None
    }
    fn hears(&self) -> Option<Vec<u8>> {
        self.next(|p| p[0] == 2 || p[0] == 3)
    }
}

fn ask_secret(s: &Server, who: u128) -> Vec<u8> {
    s.message(who, proto::REQUEST_SECRET, &20i32.to_be_bytes());
    let out = s.outgoing(64);
    for_player(&out, who)
        .into_iter()
        .find(|(c, _)| c == proto::SECRET)
        .expect("a secret")
        .1
}

#[test]
fn only_asking_clients_hear_of_voice_chat() {
    let s = server();
    assert!(s.outgoing(64).is_empty(), "nothing before anyone asks");
    // Wrong version: no answer.
    s.message(A, proto::REQUEST_SECRET, &21i32.to_be_bytes());
    assert!(s.outgoing(64).is_empty());
    s.message(A, proto::REQUEST_SECRET, &20i32.to_be_bytes());
    s.tick(world());
    let out = s.outgoing(64);
    assert!(out.iter().all(|m| m.0 == A), "{out:?}");
    let channels: Vec<String> = for_player(&out, A).into_iter().map(|m| m.0).collect();
    assert_eq!(channels, [proto::SECRET, proto::STATES, proto::STATE]);
    // A message from someone not on the server, or garbage, is ignored.
    s.message(0xF, proto::REQUEST_SECRET, &20i32.to_be_bytes());
    s.message(B, proto::CREATE_GROUP, &[200]);
    s.tick(world());
    assert!(s.outgoing(64).is_empty());
}

#[test]
fn voices_reach_who_is_near_in_the_same_dimension() {
    let s = server();
    let voices: Vec<Voice> = [A, B, FAR, NETHER]
        .iter()
        .map(|&u| Voice::connect(u, &ask_secret(&s, u)))
        .collect();
    let [a, b, far, nether] = &voices[..] else {
        unreachable!()
    };
    s.tick(world());
    let states = s.outgoing(64);
    assert!(
        for_player(&states, PLAIN).is_empty(),
        "the player without voice chat gets nothing"
    );

    a.talk(0, false);
    let heard = b.hears().expect("bob, 10 blocks away, hears alice");
    assert_eq!(heard[0], 2, "positional");
    assert_eq!(&heard[1..17], &A.to_be_bytes(), "at alice");
    assert_eq!(&heard[33..34], &[40]);
    assert_eq!(
        &heard[heard.len() - 13..heard.len() - 5],
        &0i64.to_be_bytes()
    );
    assert_eq!(far.hears(), None, "500 blocks away");
    assert_eq!(nether.hears(), None, "another dimension");
    assert_eq!(a.hears(), None, "not your own voice");

    // A whisper carries 24 blocks: bob at 10 still hears it; at 30 he doesn't.
    a.talk(1, true);
    assert_eq!(
        b.hears().map(|p| p[p.len() - 1]),
        Some(1),
        "whispering flag"
    );
    let mut moved = world();
    moved[1].position[0] = 30.0;
    s.tick(moved);
    a.talk(2, true);
    assert_eq!(b.hears(), None);
    a.talk(3, false);
    assert!(b.hears().is_some());
    s.tick(world());

    // Voice chat off: bob neither hears nor is heard.
    s.message(B, proto::UPDATE_STATE, &[1]);
    a.talk(4, false);
    assert_eq!(b.hears(), None);
    b.talk(0, false);
    assert_eq!(a.hears(), None);
    s.message(B, proto::UPDATE_STATE, &[0]);
    a.talk(5, false);
    assert!(b.hears().is_some());
}

#[test]
fn groups_follow_their_kind() {
    let s = server();
    let a = Voice::connect(A, &ask_secret(&s, A));
    let b = Voice::connect(B, &ask_secret(&s, B));
    let far = Voice::connect(FAR, &ask_secret(&s, FAR));
    s.tick(world());
    s.outgoing(64);

    // Alice makes an isolated group with a password; carol joins it from 500 blocks.
    s.message(
        A,
        proto::CREATE_GROUP,
        &Out::default()
            .text("Base")
            .flag(true)
            .text("pw")
            .short(2)
            .done(),
    );
    let out = s.outgoing(64);
    let add = for_player(&out, FAR)
        .into_iter()
        .find(|m| m.0 == proto::ADD_GROUP)
        .expect("everyone sees the group")
        .1;
    let id = In::new(&add).uuid().unwrap();
    let joined = for_player(&out, A)
        .into_iter()
        .find(|m| m.0 == proto::JOINED_GROUP)
        .unwrap()
        .1;
    assert_eq!(joined, proto::joined_group(Some(id), false));

    s.message(
        FAR,
        proto::SET_GROUP,
        &Out::default().uuid(id).flag(true).text("nope").done(),
    );
    let out = s.outgoing(64);
    assert_eq!(
        for_player(&out, FAR),
        vec![(
            proto::JOINED_GROUP.to_owned(),
            proto::joined_group(None, true)
        )]
    );
    s.message(
        FAR,
        proto::SET_GROUP,
        &Out::default().uuid(id).flag(true).text("pw").done(),
    );
    s.tick(world());
    s.outgoing(64);

    a.talk(0, false);
    assert_eq!(
        far.hears().map(|p| p[0]),
        Some(3),
        "a group voice, from anywhere"
    );
    assert_eq!(b.hears(), None, "isolated: bob nearby doesn't hear alice");
    b.talk(0, false);
    assert_eq!(a.hears(), None, "isolated: alice doesn't hear bob");

    // Both leave: the empty group goes away for everyone.
    s.message(A, proto::LEAVE_GROUP, &[]);
    s.message(FAR, proto::LEAVE_GROUP, &[]);
    let out = s.outgoing(64);
    assert!(
        for_player(&out, B).contains(&(proto::REMOVE_GROUP.to_owned(), proto::remove_group(id)))
    );
    a.talk(1, false);
    assert_eq!(b.hears().map(|p| p[0]), Some(2), "back to proximity");

    // An open group: members are heard nearby, and hear who is nearby.
    s.message(
        A,
        proto::CREATE_GROUP,
        &Out::default().text("Open").flag(false).short(1).done(),
    );
    s.tick(world());
    s.outgoing(64);
    a.talk(2, false);
    assert_eq!(b.hears().map(|p| p[0]), Some(2));
    b.talk(1, false);
    assert_eq!(a.hears().map(|p| p[0]), Some(2));
    // A normal one: members aren't heard outside, but hear who is nearby.
    s.message(
        A,
        proto::CREATE_GROUP,
        &Out::default().text("Normal").flag(false).short(0).done(),
    );
    s.tick(world());
    s.outgoing(64);
    a.talk(3, false);
    assert_eq!(b.hears(), None);
    b.talk(2, false);
    assert_eq!(a.hears().map(|p| p[0]), Some(2));
}

#[test]
fn leaving_and_strangers() {
    let s = server();
    let a = Voice::connect(A, &ask_secret(&s, A));
    let b = Voice::connect(B, &ask_secret(&s, B));
    s.tick(world());
    s.outgoing(64);

    // Someone without the key can't speak as alice, nor take her address.
    let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
    let fake = proto::Cipher::new(&[0; 16])
        .seal(&SystemRandom::new(), &[1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0])
        .unwrap();
    stranger
        .send_to(
            &Out::default()
                .byte(proto::MAGIC)
                .uuid(A)
                .bytes(&fake)
                .done(),
            b.server,
        )
        .unwrap();
    assert_eq!(b.hears(), None);
    // A copy of alice's own handshake from elsewhere, while she's active, is refused.
    let replay = a
        .cipher
        .seal(
            &a.rng,
            &Out::default()
                .byte(5)
                .uuid(A)
                .done()
                .into_iter()
                .chain(a.secret)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    stranger
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    stranger
        .send_to(
            &Out::default()
                .byte(proto::MAGIC)
                .uuid(A)
                .bytes(&replay)
                .done(),
            b.server,
        )
        .unwrap();
    let mut buf = [0u8; 64];
    assert!(
        stranger.recv_from(&mut buf).is_err(),
        "no answer to a stranger"
    );
    a.talk(0, false);
    assert!(
        b.hears().is_some(),
        "alice still talks from her own address"
    );

    // Bob leaves the game: alice is told, and his voice is gone.
    let mut without_bob = world();
    without_bob.retain(|p| p.uuid != B);
    s.tick(without_bob);
    let out = s.outgoing(64);
    assert_eq!(
        for_player(&out, A),
        vec![(proto::REMOVE_STATE.to_owned(), proto::remove_state(B))]
    );
    b.talk(5, false);
    assert_eq!(a.hears(), None);
    s.left(A);
    assert!(s.outgoing(64).is_empty());
}

#[test]
fn outgoing_is_paced_per_player() {
    let s = server();
    ask_secret(&s, A);
    for i in 0..10 {
        s.message(
            A,
            proto::CREATE_GROUP,
            &Out::default()
                .text(&format!("g{i}"))
                .flag(false)
                .short(0)
                .done(),
        );
    }
    let first = s.outgoing(4);
    assert_eq!(first.len(), 4);
    let mut rest = 0;
    loop {
        let more = s.outgoing(4);
        if more.is_empty() {
            break;
        }
        assert!(more.len() <= 4);
        rest += more.len();
    }
    // Each group: its add and alice's joined (the old one, emptied, is removed).
    assert_eq!(4 + rest, 10 * 2 + 9);
}

#[test]
fn config_defaults_fill_in() {
    let c: Config = serde_json::from_str(r#"{"port": 25565}"#).unwrap();
    assert_eq!((c.port, c.distance, c.enabled), (25565, 48.0, true));
    assert!(
        Server::start(Config {
            enabled: false,
            ..Config::default()
        })
        .is_err()
    );
}

/// A socket elsewhere that copies datagrams it saw and sends them as its own.
fn stranger() -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    socket
}

fn answered(socket: &UdpSocket) -> bool {
    let mut buf = [0u8; 4096];
    socket.recv_from(&mut buf).is_ok()
}

#[test]
fn a_replayed_handshake_after_a_silence_moves_nothing() {
    let s = server();
    let a = Voice::connect(A, &ask_secret(&s, A));
    let b = Voice::connect(B, &ask_secret(&s, B));
    s.tick(world());
    s.outgoing(64);

    // Alice says nothing for longer than the old 5 s move rule (but less than
    // the 10 s after which she counts as lost).
    std::thread::sleep(Duration::from_millis(5300));
    let eve = stranger();
    for datagram in &a.handshake {
        eve.send_to(datagram, a.server).unwrap();
        assert!(!answered(&eve), "no answer to a copied handshake");
    }
    b.talk(0, false);
    assert!(a.hears().is_some(), "bob's voice still goes to alice");
    assert!(!answered(&eve), "and not to the copier");

    // Exact copies from alice's own address are dropped too.
    a.socket.send_to(&a.handshake[1], a.server).unwrap();
    assert_eq!(a.next(|p| p == [10]), None, "a copied check isn't answered");
    let said = a.talk(0, false);
    assert!(b.hears().is_some());
    a.socket.send_to(&said, a.server).unwrap();
    assert_eq!(b.hears(), None, "a copied voice packet isn't relayed twice");
    a.talk(1, false);
    assert!(b.hears().is_some(), "the next one is");

    // Once her handshake has left the nonces remembered, the secret still
    // belongs to her address alone.
    for _ in 0..SEEN_NONCES + 4 {
        a.send(&[8]);
    }
    a.idle(200);
    for datagram in &a.handshake {
        eve.send_to(datagram, a.server).unwrap();
        assert!(!answered(&eve), "no answer to an old copy either");
    }
    b.talk(1, false);
    assert!(a.hears().is_some());
    assert!(!answered(&eve));
}

#[test]
fn a_client_the_server_stops_hearing_gets_a_fresh_secret() {
    let s = server_with(Config {
        // Lost after 3 s: bob, who answers keep-alives only while a test
        // reads his socket, must not count as lost too.
        keep_alive_ms: 300,
        ..Config::default()
    });
    let a = Voice::connect(A, &ask_secret(&s, A));
    let b = Voice::connect(B, &ask_secret(&s, B));
    s.message(
        A,
        proto::CREATE_GROUP,
        &Out::default().text("Open").flag(false).short(1).done(),
    );
    s.tick(world());
    let out = s.outgoing(64);
    let group = for_player(&out, A)
        .into_iter()
        .find(|m| m.0 == proto::ADD_GROUP)
        .map(|m| In::new(&m.1).uuid().unwrap())
        .unwrap();

    // Alice's NAT rebinds: the server hears nothing from her for 10 keep-alives.
    b.idle(3500);
    let out = s.outgoing(64);
    let to_alice = for_player(&out, A);
    assert_eq!(
        to_alice.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(),
        [proto::SECRET],
        "only a new secret, over the game connection"
    );
    let fresh = &to_alice[0].1;
    assert_ne!(&fresh[..16], &a.secret, "a new key");

    // The old key opens nothing now, from anywhere.
    let eve = stranger();
    eve.send_to(&a.handshake[0], a.server).unwrap();
    assert!(!answered(&eve));
    a.send(&[9]);
    assert_eq!(a.next(|p| p == [10]), None);

    // From her new address with the new secret, she is back, in her group.
    let moved = Voice::connect(A, fresh);
    s.tick(world());
    let out = s.outgoing(64);
    let alice = proto::state(&PlayerState {
        uuid: A,
        name: "alice".into(),
        disabled: false,
        disconnected: false,
        group: Some(group),
    });
    assert!(
        for_player(&out, B).contains(&(proto::STATE.to_owned(), alice)),
        "{out:?}"
    );
    b.talk(0, false);
    assert!(
        moved.hears().is_some(),
        "bob's voice reaches her new address"
    );
    assert_eq!(a.hears(), None, "and not the old one");
    moved.talk(0, false);
    assert!(b.hears().is_some());
}

#[test]
fn repeated_secret_requests_and_group_changes_stay_bounded() {
    let s = server();
    // 40 players, each in a group of their own.
    let many: Vec<Player> = (0..40u128)
        .map(|i| player(0x100 + i, &format!("p{i}"), "minecraft:overworld", 0.0))
        .collect();
    s.tick(many.clone());
    for p in &many {
        ask_secret(&s, p.uuid);
        s.message(
            p.uuid,
            proto::CREATE_GROUP,
            &Out::default().text(&p.name).flag(false).short(0).done(),
        );
    }
    s.tick(many.clone());
    s.outgoing(usize::MAX);

    // A thousand requests in a row: answered once now, once more a second later.
    let first = many[0].uuid;
    for _ in 0..1000 {
        s.message(first, proto::REQUEST_SECRET, &20i32.to_be_bytes());
    }
    let count = |out: &[Message], channel: &str| {
        out.iter()
            .filter(|m| m.0 == first && m.1 == channel)
            .count()
    };
    let out = s.outgoing(usize::MAX);
    assert_eq!(count(&out, proto::SECRET), 0, "too soon after the last one");
    std::thread::sleep(SECRET_EVERY + Duration::from_millis(50));
    s.tick(many.clone());
    let out = s.outgoing(usize::MAX);
    assert_eq!(count(&out, proto::SECRET), 1);
    // Starting over, it left its own group, which went away.
    assert_eq!(count(&out, proto::ADD_GROUP), 39, "the groups once");
    let mine: Vec<String> = for_player(&out, first).into_iter().map(|m| m.0).collect();
    assert!(mine.len() <= 1 + 1 + 39 + 1, "{mine:?}");
    s.tick(many.clone());
    assert_eq!(count(&s.outgoing(usize::MAX), proto::SECRET), 0);

    // Group changes: a burst of 10, then 2 a second.
    let second = many[1].uuid;
    for i in 0..100 {
        s.message(
            second,
            proto::CREATE_GROUP,
            &Out::default()
                .text(&format!("g{i}"))
                .flag(false)
                .short(0)
                .done(),
        );
    }
    let out = s.outgoing(usize::MAX);
    let joined = out
        .iter()
        .filter(|m| m.0 == second && m.1 == proto::JOINED_GROUP)
        .count();
    // Its bucket refilled during the wait: 10 of the 100.
    assert_eq!(joined, OPS_BURST as usize);
}
