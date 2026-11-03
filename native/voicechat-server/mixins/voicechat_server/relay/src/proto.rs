//! The voice chat wire format Simple Voice Chat 2.6.x clients speak
//! (compatibility version 20), from the server's side: control messages on
//! `voicechat:` channels over the game connection, and AES-128-GCM UDP
//! datagrams for the voice itself.
//!
//! Numbers are big-endian, a UUID is 16 bytes (most significant half first),
//! strings and byte arrays are a VarInt length then the bytes.

/// The only version this server speaks.
pub const COMPATIBILITY_VERSION: i32 = 20;
/// Every UDP datagram starts with it.
pub const MAGIC: u8 = 0xFF;
/// The largest Opus frame relayed.
pub const MAX_OPUS: usize = 1275;
/// The largest encrypted body read from a datagram.
pub const MAX_BODY: usize = 2048;
/// Group names and passwords, in characters.
pub const MAX_GROUP_TEXT: usize = 24;

pub const REQUEST_SECRET: &str = "voicechat:request_secret";
pub const SECRET: &str = "voicechat:secret";
pub const UPDATE_STATE: &str = "voicechat:update_state";
pub const STATE: &str = "voicechat:state";
pub const STATES: &str = "voicechat:states";
pub const REMOVE_STATE: &str = "voicechat:remove_state";
pub const CREATE_GROUP: &str = "voicechat:create_group";
pub const SET_GROUP: &str = "voicechat:set_group";
pub const LEAVE_GROUP: &str = "voicechat:leave_group";
pub const JOINED_GROUP: &str = "voicechat:joined_group";
pub const ADD_GROUP: &str = "voicechat:add_group";
pub const REMOVE_GROUP: &str = "voicechat:remove_group";

/// A message that can't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed;

type Read<T> = Result<T, Malformed>;

pub struct In<'a> {
    rest: &'a [u8],
}

impl<'a> In<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { rest: data }
    }
    pub fn take(&mut self, n: usize) -> Read<&'a [u8]> {
        if self.rest.len() < n {
            return Err(Malformed);
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }
    fn fixed<const N: usize>(&mut self) -> Read<[u8; N]> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
    pub fn byte(&mut self) -> Read<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn flag(&mut self) -> Read<bool> {
        Ok(self.byte()? != 0)
    }
    pub fn short(&mut self) -> Read<i16> {
        Ok(i16::from_be_bytes(self.fixed()?))
    }
    pub fn int(&mut self) -> Read<i32> {
        Ok(i32::from_be_bytes(self.fixed()?))
    }
    pub fn long(&mut self) -> Read<i64> {
        Ok(i64::from_be_bytes(self.fixed()?))
    }
    pub fn uuid(&mut self) -> Read<u128> {
        Ok(u128::from_be_bytes(self.fixed()?))
    }
    pub fn varint(&mut self) -> Read<i32> {
        let mut value = 0u32;
        for shift in 0..5 {
            let b = self.byte()?;
            value |= u32::from(b & 0x7F) << (shift * 7);
            if b & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(Malformed)
    }
    /// A VarInt length of at most `max`, then that many bytes.
    pub fn bytes(&mut self, max: usize) -> Read<&'a [u8]> {
        let n = usize::try_from(self.varint()?).map_err(|_| Malformed)?;
        if n > max {
            return Err(Malformed);
        }
        self.take(n)
    }
    /// A string of at most `max` characters.
    pub fn text(&mut self, max: usize) -> Read<String> {
        let raw = self.bytes(max.saturating_mul(4))?;
        let s = std::str::from_utf8(raw).map_err(|_| Malformed)?;
        if s.chars().count() > max {
            return Err(Malformed);
        }
        Ok(s.to_owned())
    }
}

#[derive(Default)]
pub struct Out(pub Vec<u8>);

impl Out {
    pub fn byte(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn flag(&mut self, v: bool) -> &mut Self {
        self.byte(u8::from(v))
    }
    pub fn short(&mut self, v: i16) -> &mut Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn int(&mut self, v: i32) -> &mut Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn long(&mut self, v: i64) -> &mut Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn float(&mut self, v: f32) -> &mut Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn double(&mut self, v: f64) -> &mut Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn uuid(&mut self, v: u128) -> &mut Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn varint(&mut self, v: i32) -> &mut Self {
        let mut v = v as u32;
        while v >= 0x80 {
            self.0.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.0.push(v as u8);
        self
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.varint(b.len() as i32);
        self.0.extend_from_slice(b);
        self
    }
    pub fn text(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }
    pub fn done(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

// ------------------------------------------------------------ the game connection

/// How a group hears the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    /// Members hear each other anywhere and players nearby; nobody outside hears them.
    Normal,
    /// Members hear each other anywhere; players nearby hear them and are heard.
    Open,
    /// Members hear only each other.
    Isolated,
}

impl GroupKind {
    pub fn from_wire(v: i16) -> Self {
        match v {
            1 => Self::Open,
            2 => Self::Isolated,
            _ => Self::Normal,
        }
    }
    pub fn wire(self) -> i16 {
        match self {
            Self::Normal => 0,
            Self::Open => 1,
            Self::Isolated => 2,
        }
    }
}

/// What a client asks on a `voicechat:` channel.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Secret {
        version: i32,
    },
    State {
        disabled: bool,
    },
    CreateGroup {
        name: String,
        password: Option<String>,
        kind: GroupKind,
    },
    JoinGroup {
        group: u128,
        password: Option<String>,
    },
    LeaveGroup,
}

impl Request {
    /// None: not a channel a client sends on.
    pub fn read(channel: &str, data: &[u8]) -> Option<Read<Self>> {
        let mut r = In::new(data);
        let parsed = match channel {
            REQUEST_SECRET => r.int().map(|version| Self::Secret { version }),
            UPDATE_STATE => r.flag().map(|disabled| Self::State { disabled }),
            CREATE_GROUP => (|| {
                let name = r.text(MAX_GROUP_TEXT)?;
                let password = if r.flag()? {
                    Some(r.text(MAX_GROUP_TEXT)?)
                } else {
                    None
                };
                let kind = GroupKind::from_wire(r.short()?);
                Ok(Self::CreateGroup {
                    name,
                    password,
                    kind,
                })
            })(),
            SET_GROUP => (|| {
                let group = r.uuid()?;
                let password = if r.flag()? {
                    Some(r.text(MAX_GROUP_TEXT)?)
                } else {
                    None
                };
                Ok(Self::JoinGroup { group, password })
            })(),
            LEAVE_GROUP => Ok(Self::LeaveGroup),
            _ => return None,
        };
        Some(parsed)
    }
}

/// `voicechat:secret`: what the client needs to open its voice connection.
pub struct SecretInfo<'a> {
    pub secret: &'a [u8; 16],
    pub port: u16,
    pub player: u128,
    pub mtu: i32,
    pub distance: f64,
    pub keep_alive_ms: i32,
    pub groups: bool,
    /// Empty: the game server's own address.
    pub voice_host: &'a str,
}

pub fn secret(s: &SecretInfo) -> Vec<u8> {
    let mut o = Out::default();
    o.0.extend_from_slice(s.secret);
    o.int(i32::from(s.port))
        .uuid(s.player)
        .byte(0) // the Opus application: VoIP
        .int(s.mtu)
        .double(s.distance)
        .int(s.keep_alive_ms)
        .flag(s.groups)
        .text(s.voice_host)
        .flag(false) // recording not allowed
        .done()
}

/// A player's voice chat state, as `voicechat:state` and each of `voicechat:states` carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerState {
    pub uuid: u128,
    pub name: String,
    pub disabled: bool,
    pub disconnected: bool,
    pub group: Option<u128>,
}

fn put_state(o: &mut Out, s: &PlayerState) {
    o.flag(s.disabled)
        .flag(s.disconnected)
        .uuid(s.uuid)
        .text(&s.name)
        .flag(s.group.is_some());
    if let Some(g) = s.group {
        o.uuid(g);
    }
}

pub fn state(s: &PlayerState) -> Vec<u8> {
    let mut o = Out::default();
    put_state(&mut o, s);
    o.done()
}

pub fn states(all: &[PlayerState]) -> Vec<u8> {
    let mut o = Out::default();
    o.int(all.len() as i32);
    for s in all {
        put_state(&mut o, s);
    }
    o.done()
}

pub fn remove_state(uuid: u128) -> Vec<u8> {
    Out::default().uuid(uuid).done()
}

pub fn add_group(id: u128, name: &str, has_password: bool, kind: GroupKind) -> Vec<u8> {
    Out::default()
        .uuid(id)
        .text(name)
        .flag(has_password)
        .flag(false) // persistent
        .flag(false) // hidden
        .short(kind.wire())
        .done()
}

pub fn remove_group(id: u128) -> Vec<u8> {
    Out::default().uuid(id).done()
}

pub fn joined_group(group: Option<u128>, wrong_password: bool) -> Vec<u8> {
    let mut o = Out::default();
    o.flag(group.is_some());
    if let Some(g) = group {
        o.uuid(g);
    }
    o.flag(wrong_password).done()
}

// ------------------------------------------------------------ UDP

/// A voice packet a client sends.
#[derive(Debug, Clone, PartialEq)]
pub enum FromClient {
    /// The microphone: an Opus frame (empty: stopped talking).
    Mic {
        data: Vec<u8>,
        sequence: i64,
        whispering: bool,
    },
    Authenticate {
        player: u128,
        secret: [u8; 16],
    },
    Ping {
        id: u128,
        timestamp: i64,
    },
    KeepAlive,
    ConnectionCheck,
}

impl FromClient {
    pub fn read(plain: &[u8]) -> Read<Self> {
        let mut r = In::new(plain);
        Ok(match r.byte()? {
            1 => Self::Mic {
                data: r.bytes(MAX_OPUS)?.to_vec(),
                sequence: r.long()?,
                whispering: r.flag()?,
            },
            5 => Self::Authenticate {
                player: r.uuid()?,
                secret: r.fixed()?,
            },
            7 => Self::Ping {
                id: r.uuid()?,
                timestamp: r.long()?,
            },
            8 => Self::KeepAlive,
            9 => Self::ConnectionCheck,
            _ => return Err(Malformed),
        })
    }
}

/// A voice packet this server sends.
#[derive(Debug, Clone, PartialEq)]
pub enum ToClient<'a> {
    /// A voice heard where `sender` stands.
    Positional {
        sender: u128,
        data: &'a [u8],
        sequence: i64,
        distance: f32,
        whispering: bool,
    },
    /// A group member's voice, heard everywhere.
    Group {
        sender: u128,
        data: &'a [u8],
        sequence: i64,
    },
    AuthenticateAck,
    Ping {
        id: u128,
        timestamp: i64,
    },
    KeepAlive,
    ConnectionCheckAck,
}

impl ToClient<'_> {
    pub fn write(&self) -> Vec<u8> {
        let mut o = Out::default();
        match self {
            // The channel is the sender: the client places a positional voice at that entity.
            Self::Positional {
                sender,
                data,
                sequence,
                distance,
                whispering,
            } => o
                .byte(2)
                .uuid(*sender)
                .uuid(*sender)
                .bytes(data)
                .long(*sequence)
                .float(*distance)
                .flag(*whispering),
            Self::Group {
                sender,
                data,
                sequence,
            } => o
                .byte(3)
                .uuid(*sender)
                .uuid(*sender)
                .bytes(data)
                .long(*sequence)
                .byte(0),
            Self::AuthenticateAck => o.byte(6),
            Self::Ping { id, timestamp } => o.byte(7).uuid(*id).long(*timestamp),
            Self::KeepAlive => o.byte(8),
            Self::ConnectionCheckAck => o.byte(10),
        };
        o.done()
    }
}

/// AES-128-GCM keyed by a player's secret: a random 12-byte nonce, then the
/// ciphertext and its 16-byte tag.
pub struct Cipher {
    key: ring::aead::LessSafeKey,
}

impl Cipher {
    pub fn new(secret: &[u8; 16]) -> Self {
        let key =
            ring::aead::UnboundKey::new(&ring::aead::AES_128_GCM, secret).expect("a 16-byte key");
        Self {
            key: ring::aead::LessSafeKey::new(key),
        }
    }
    pub fn seal(&self, rng: &dyn ring::rand::SecureRandom, plain: &[u8]) -> Option<Vec<u8>> {
        let mut nonce = [0u8; 12];
        rng.fill(&mut nonce).ok()?;
        let mut body = plain.to_vec();
        self.key
            .seal_in_place_append_tag(
                ring::aead::Nonce::assume_unique_for_key(nonce),
                ring::aead::Aad::empty(),
                &mut body,
            )
            .ok()?;
        let mut out = nonce.to_vec();
        out.extend(body);
        Some(out)
    }
    pub fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < 12 + 16 {
            return None;
        }
        let (nonce, body) = sealed.split_at(12);
        let nonce = ring::aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
        let mut body = body.to_vec();
        let plain = self
            .key
            .open_in_place(nonce, ring::aead::Aad::empty(), &mut body)
            .ok()?;
        Some(plain.to_vec())
    }
}

/// A client's datagram: magic, the player's UUID (in clear, to find their key), the sealed packet.
pub fn client_datagram(datagram: &[u8]) -> Read<(u128, &[u8])> {
    let mut r = In::new(datagram);
    if r.byte()? != MAGIC {
        return Err(Malformed);
    }
    let player = r.uuid()?;
    Ok((player, r.bytes(MAX_BODY)?))
}

/// A server datagram: magic, then the sealed packet.
pub fn server_datagram(sealed: &[u8]) -> Vec<u8> {
    Out::default().byte(MAGIC).bytes(sealed).done()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_match_minecraft() {
        for (v, bytes) in [
            (0, vec![0]),
            (127, vec![0x7F]),
            (128, vec![0x80, 1]),
            (300, vec![0xAC, 2]),
            (-1, vec![0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        ] {
            assert_eq!(Out::default().varint(v).done(), bytes, "{v}");
            assert_eq!(In::new(&bytes).varint().unwrap(), v);
        }
        assert_eq!(In::new(&[0x80; 6]).varint(), Err(Malformed));
    }

    #[test]
    fn requests_read_their_layouts() {
        assert_eq!(
            Request::read(REQUEST_SECRET, &[0, 0, 0, 20]),
            Some(Ok(Request::Secret { version: 20 }))
        );
        assert_eq!(
            Request::read(UPDATE_STATE, &[1]),
            Some(Ok(Request::State { disabled: true }))
        );
        // name "ab", a password "x", kind open.
        assert_eq!(
            Request::read(CREATE_GROUP, &[2, b'a', b'b', 1, 1, b'x', 0, 1]),
            Some(Ok(Request::CreateGroup {
                name: "ab".into(),
                password: Some("x".into()),
                kind: GroupKind::Open
            }))
        );
        let mut join = 7u128.to_be_bytes().to_vec();
        join.push(0);
        assert_eq!(
            Request::read(SET_GROUP, &join),
            Some(Ok(Request::JoinGroup {
                group: 7,
                password: None
            }))
        );
        assert_eq!(
            Request::read(LEAVE_GROUP, &[]),
            Some(Ok(Request::LeaveGroup))
        );
        assert_eq!(
            Request::read("voicechat:secret", &[]),
            None,
            "a server's channel"
        );
        let long = [&[30u8][..], &[b'n'; 30]].concat();
        assert_eq!(
            Request::read(CREATE_GROUP, &[&long[..], &[0, 0, 0]].concat()),
            Some(Err(Malformed))
        );
        assert_eq!(Request::read(REQUEST_SECRET, &[0, 0]), Some(Err(Malformed)));
    }

    #[test]
    fn secret_layout() {
        let key = [9u8; 16];
        let bytes = secret(&SecretInfo {
            secret: &key,
            port: 24454,
            player: 0xAB,
            mtu: 1024,
            distance: 48.0,
            keep_alive_ms: 1000,
            groups: true,
            voice_host: "",
        });
        // key, port, uuid, codec, mtu, distance, keep-alive, groups, "" , recording
        assert_eq!(bytes.len(), 16 + 4 + 16 + 1 + 4 + 8 + 4 + 1 + 1 + 1);
        assert_eq!(&bytes[..16], &key);
        assert_eq!(&bytes[16..20], &24454i32.to_be_bytes());
        assert_eq!(&bytes[20..36], &0xABu128.to_be_bytes());
        assert_eq!(&bytes[41..49], &48.0f64.to_be_bytes());
        assert_eq!(bytes[53], 1);
    }

    #[test]
    fn states_and_groups_layouts() {
        let a = PlayerState {
            uuid: 1,
            name: "a".into(),
            disabled: true,
            disconnected: false,
            group: Some(2),
        };
        assert_eq!(
            state(&a),
            [
                &[1u8, 0][..],
                &1u128.to_be_bytes(),
                &[1, b'a', 1],
                &2u128.to_be_bytes()
            ]
            .concat()
        );
        let both = states(&[
            a.clone(),
            PlayerState {
                group: None,
                ..a.clone()
            },
        ]);
        assert_eq!(&both[..4], &2i32.to_be_bytes());
        assert_eq!(both.len(), 4 + 37 + 21);
        let g = add_group(5, "Base", true, GroupKind::Isolated);
        assert_eq!(
            g,
            [
                &5u128.to_be_bytes()[..],
                &[4, b'B', b'a', b's', b'e', 1, 0, 0, 0, 2]
            ]
            .concat()
        );
        assert_eq!(joined_group(None, true), vec![0, 1]);
        assert_eq!(
            joined_group(Some(3), false),
            [&[1u8][..], &3u128.to_be_bytes(), &[0]].concat()
        );
    }

    #[test]
    fn voice_packets() {
        let mic = [&[1u8, 2, 0xAA, 0xBB][..], &5i64.to_be_bytes(), &[1]].concat();
        assert_eq!(
            FromClient::read(&mic),
            Ok(FromClient::Mic {
                data: vec![0xAA, 0xBB],
                sequence: 5,
                whispering: true
            })
        );
        let auth = [&[5u8][..], &7u128.to_be_bytes(), &[3; 16]].concat();
        assert_eq!(
            FromClient::read(&auth),
            Ok(FromClient::Authenticate {
                player: 7,
                secret: [3; 16]
            })
        );
        assert_eq!(FromClient::read(&[8]), Ok(FromClient::KeepAlive));
        assert_eq!(FromClient::read(&[9]), Ok(FromClient::ConnectionCheck));
        assert_eq!(FromClient::read(&[2]), Err(Malformed), "a server's packet");
        let big = [
            &[1u8][..],
            &Out::default().varint(1276).done(),
            &[0; 1276],
            &[0; 9],
        ]
        .concat();
        assert_eq!(FromClient::read(&big), Err(Malformed));

        let sound = ToClient::Positional {
            sender: 4,
            data: &[1, 2],
            sequence: 9,
            distance: 48.0,
            whispering: false,
        }
        .write();
        assert_eq!(sound[0], 2);
        assert_eq!(&sound[1..17], &4u128.to_be_bytes());
        assert_eq!(&sound[17..33], &4u128.to_be_bytes());
        assert_eq!(&sound[33..36], &[2, 1, 2]);
        assert_eq!(&sound[36..44], &9i64.to_be_bytes());
        assert_eq!(&sound[44..48], &48.0f32.to_be_bytes());
        assert_eq!(sound[48], 0);
        assert_eq!(sound.len(), 49);
        let group = ToClient::Group {
            sender: 4,
            data: &[1],
            sequence: 2,
        }
        .write();
        assert_eq!(
            (group[0], group.len(), group[group.len() - 1]),
            (3, 1 + 32 + 2 + 8 + 1, 0)
        );
        assert_eq!(ToClient::AuthenticateAck.write(), vec![6]);
        assert_eq!(ToClient::ConnectionCheckAck.write(), vec![10]);
    }

    #[test]
    fn sealed_bodies_round_trip_and_reject_tampering() {
        let rng = ring::rand::SystemRandom::new();
        let c = Cipher::new(&[1; 16]);
        let sealed = c.seal(&rng, b"hello").unwrap();
        assert_eq!(sealed.len(), 12 + 5 + 16);
        assert_eq!(c.open(&sealed).unwrap(), b"hello");
        let mut bad = sealed.clone();
        bad[14] ^= 1;
        assert_eq!(c.open(&bad), None);
        assert_eq!(Cipher::new(&[2; 16]).open(&sealed), None);
        assert_ne!(
            c.seal(&rng, b"hello").unwrap(),
            sealed,
            "fresh nonce each time"
        );

        let datagram = [
            &[MAGIC][..],
            &9u128.to_be_bytes(),
            &Out::default().bytes(&sealed).done(),
        ]
        .concat();
        assert_eq!(client_datagram(&datagram), Ok((9, &sealed[..])));
        assert_eq!(client_datagram(&datagram[1..]), Err(Malformed));
        assert_eq!(server_datagram(&[1, 2])[..], [MAGIC, 2, 1, 2]);
    }
}
