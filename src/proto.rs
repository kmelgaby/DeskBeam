//! DeskBeam wire protocol: framing, chunking, and message schema.
//!
//! See `docs/PROTOCOL.md` for the normative specification.

use anyhow::{bail, Context, Result};
use bincode::Options;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 0;
pub const MAGIC: [u8; 4] = *b"DBM1";

/// Maximum ciphertext record payload.
pub const MAX_RECORD: usize = 65_535;
/// Maximum plaintext handshake record payload.
pub const MAX_HANDSHAKE_RECORD: usize = 4_096;
/// Maximum plaintext bytes in one encrypted chunk (including its 10-byte header).
pub const MAX_CHUNK_PLAINTEXT: usize = 60_000;
/// Chunk header: type(2) + msg_id(4) + index(2) + count(2) = 10 bytes.
pub const CHUNK_HEADER: usize = 10;
/// Maximum logical message size.
pub const MAX_LOGICAL: usize = 16 * 1024 * 1024;
/// Maximum chunks per logical message.
pub const MAX_CHUNKS: u16 = 4_096;
/// Drop incomplete messages after this long.
pub const PARTIAL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum MsgType {
    ClientHello = 0x0001,
    ServerHello = 0x0002,
    SpakeA = 0x0010,
    SpakeB = 0x0011,
    NoiseHandshake = 0x0020,
    Close = 0x00F0,
    AuthFailed = 0x00F1,

    Hello = 0x0100,
    AcceptRequest = 0x0101,
    Accept = 0x0102,
    Reject = 0x0103,
    SessionInfo = 0x0104,
    VideoFrame = 0x0110,
    InputEvent = 0x0120,
    SelectMonitor = 0x0121,
    ClipboardText = 0x0130,
    ClipboardRequest = 0x0131,
    Ping = 0x0140,
    Pong = 0x0141,
    Stats = 0x0150,
    RequestKeyframe = 0x0151,
    Bye = 0x0160,
    Chat = 0x0170,
    FileOffer = 0x0180,
    FileAccept = 0x0181,
    FileChunk = 0x0182,
    FileDone = 0x0183,
    FileCancel = 0x0184,
}

impl MsgType {
    pub fn code(self) -> u16 {
        self as u16
    }

    pub fn from_code(code: u16) -> Option<Self> {
        use MsgType::*;
        Some(match code {
            0x0001 => ClientHello,
            0x0002 => ServerHello,
            0x0010 => SpakeA,
            0x0011 => SpakeB,
            0x0020 => NoiseHandshake,
            0x00F0 => Close,
            0x00F1 => AuthFailed,
            0x0100 => Hello,
            0x0101 => AcceptRequest,
            0x0102 => Accept,
            0x0103 => Reject,
            0x0104 => SessionInfo,
            0x0110 => VideoFrame,
            0x0120 => InputEvent,
            0x0121 => SelectMonitor,
            0x0130 => ClipboardText,
            0x0131 => ClipboardRequest,
            0x0140 => Ping,
            0x0141 => Pong,
            0x0150 => Stats,
            0x0151 => RequestKeyframe,
            0x0160 => Bye,
            0x0170 => Chat,
            0x0180 => FileOffer,
            0x0181 => FileAccept,
            0x0182 => FileChunk,
            0x0183 => FileDone,
            0x0184 => FileCancel,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelloBody {
    pub magic: [u8; 4],
    pub major: u16,
    pub minor: u16,
    pub peer_label: String,
}

impl HelloBody {
    pub fn new(peer_label: impl Into<String>) -> Self {
        Self {
            magic: MAGIC,
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            peer_label: peer_label.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloseBody {
    pub code: u16,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthFailedBody {
    pub code: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViewerHello {
    pub client_name: String,
    pub platform: String,
    pub capabilities: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptRequestBody {
    pub session_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptBody {
    pub session_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RejectBody {
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    Vp8 = 0,
    Vp9 = 1,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u16,
    pub monitors: Vec<MonitorInfo>,
    pub codec: Codec,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoFrame {
    pub seq: u64,
    pub pts_ms: u64,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputMessage {
    pub event: InputEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { nx: u16, ny: u16 },
    MouseButton { button: u8, down: bool },
    Wheel { dx: i16, dy: i16 },
    Key { vk: u16, down: bool, extended: bool },
    Text { text: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PingBody {
    pub nonce: u64,
    pub sent_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatsBody {
    pub recv_fps: u16,
    pub kbps: u32,
    pub rtt_ms: u16,
    pub decode_ms: u16,
    pub want_keyframe: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ByeBody {
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SelectMonitorBody {
    pub id: u32,
}

// ---------------------------------------------------------------------------
// Message enum
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Message {
    ClientHello(HelloBody),
    ServerHello(HelloBody),
    SpakeA(Vec<u8>),
    SpakeB(Vec<u8>),
    NoiseHandshake(Vec<u8>),
    Close(CloseBody),
    AuthFailed(AuthFailedBody),
    Hello(ViewerHello),
    AcceptRequest(AcceptRequestBody),
    Accept(AcceptBody),
    Reject(RejectBody),
    SessionInfo(SessionInfo),
    VideoFrame(VideoFrame),
    Input(InputMessage),
    SelectMonitor(SelectMonitorBody),
    Ping(PingBody),
    Pong(PingBody),
    Stats(StatsBody),
    RequestKeyframe,
    Bye(ByeBody),
}

impl Message {
    pub fn msg_type(&self) -> MsgType {
        match self {
            Message::ClientHello(_) => MsgType::ClientHello,
            Message::ServerHello(_) => MsgType::ServerHello,
            Message::SpakeA(_) => MsgType::SpakeA,
            Message::SpakeB(_) => MsgType::SpakeB,
            Message::NoiseHandshake(_) => MsgType::NoiseHandshake,
            Message::Close(_) => MsgType::Close,
            Message::AuthFailed(_) => MsgType::AuthFailed,
            Message::Hello(_) => MsgType::Hello,
            Message::AcceptRequest(_) => MsgType::AcceptRequest,
            Message::Accept(_) => MsgType::Accept,
            Message::Reject(_) => MsgType::Reject,
            Message::SessionInfo(_) => MsgType::SessionInfo,
            Message::VideoFrame(_) => MsgType::VideoFrame,
            Message::Input(_) => MsgType::InputEvent,
            Message::SelectMonitor(_) => MsgType::SelectMonitor,
            Message::Ping(_) => MsgType::Ping,
            Message::Pong(_) => MsgType::Pong,
            Message::Stats(_) => MsgType::Stats,
            Message::RequestKeyframe => MsgType::RequestKeyframe,
            Message::Bye(_) => MsgType::Bye,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(match self {
            Message::ClientHello(v) | Message::ServerHello(v) => enc(v)?,
            Message::SpakeA(v) | Message::SpakeB(v) | Message::NoiseHandshake(v) => v.clone(),
            Message::Close(v) => enc(v)?,
            Message::AuthFailed(v) => enc(v)?,
            Message::Hello(v) => enc(v)?,
            Message::AcceptRequest(v) => enc(v)?,
            Message::Accept(v) => enc(v)?,
            Message::Reject(v) => enc(v)?,
            Message::SessionInfo(v) => enc(v)?,
            Message::VideoFrame(v) => enc(v)?,
            Message::Input(v) => enc(v)?,
            Message::SelectMonitor(v) => enc(v)?,
            Message::Ping(v) | Message::Pong(v) => enc(v)?,
            Message::Stats(v) => enc(v)?,
            Message::RequestKeyframe => Vec::new(),
            Message::Bye(v) => enc(v)?,
        })
    }

    pub fn decode(msg_type: MsgType, body: &[u8]) -> Result<Message> {
        Ok(match msg_type {
            MsgType::ClientHello => Message::ClientHello(dec(body)?),
            MsgType::ServerHello => Message::ServerHello(dec(body)?),
            MsgType::SpakeA => Message::SpakeA(body.to_vec()),
            MsgType::SpakeB => Message::SpakeB(body.to_vec()),
            MsgType::NoiseHandshake => Message::NoiseHandshake(body.to_vec()),
            MsgType::Close => Message::Close(dec(body)?),
            MsgType::AuthFailed => Message::AuthFailed(dec(body)?),
            MsgType::Hello => Message::Hello(dec(body)?),
            MsgType::AcceptRequest => Message::AcceptRequest(dec(body)?),
            MsgType::Accept => Message::Accept(dec(body)?),
            MsgType::Reject => Message::Reject(dec(body)?),
            MsgType::SessionInfo => Message::SessionInfo(dec(body)?),
            MsgType::VideoFrame => Message::VideoFrame(dec(body)?),
            MsgType::InputEvent => Message::Input(dec(body)?),
            MsgType::SelectMonitor => Message::SelectMonitor(dec(body)?),
            MsgType::Ping => Message::Ping(dec(body)?),
            MsgType::Pong => Message::Pong(dec(body)?),
            MsgType::Stats => Message::Stats(dec(body)?),
            MsgType::RequestKeyframe => Message::RequestKeyframe,
            MsgType::Bye => Message::Bye(dec(body)?),
            other => bail!("unsupported message type {other:?}"),
        })
    }
}

fn enc<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(MAX_LOGICAL as u64 + 4096)
        .serialize(value)
        .context("serialize message body")
}

fn dec<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(MAX_LOGICAL as u64 + 4096)
        .deserialize(body)
        .context("deserialize message body")
}

// ---------------------------------------------------------------------------
// Plaintext handshake framing
// ---------------------------------------------------------------------------

/// Write a plaintext handshake record: `u32 BE len | u16 BE type | body`.
pub fn write_plain<W: Write>(w: &mut W, msg_type: MsgType, body: &[u8]) -> Result<()> {
    let len = 2 + body.len();
    if len > MAX_HANDSHAKE_RECORD {
        bail!("handshake record too large: {len} bytes");
    }
    w.write_all(&(len as u32).to_be_bytes())?;
    w.write_all(&msg_type.code().to_be_bytes())?;
    w.write_all(body)?;
    w.flush()?;
    Ok(())
}

/// Read one plaintext handshake record.
pub fn read_plain<R: Read>(r: &mut R) -> Result<(MsgType, Vec<u8>)> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).context("read handshake length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if !(2..=MAX_HANDSHAKE_RECORD).contains(&len) {
        bail!("invalid handshake record length {len}");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).context("read handshake body")?;
    let code = u16::from_be_bytes([buf[0], buf[1]]);
    let msg_type = MsgType::from_code(code).with_context(|| format!("unknown handshake type 0x{code:04x}"))?;
    Ok((msg_type, buf[2..].to_vec()))
}

/// Write one encrypted record: `u32 BE len | ciphertext`.
pub fn write_record<W: Write>(w: &mut W, ciphertext: &[u8]) -> Result<()> {
    if ciphertext.len() > MAX_RECORD {
        bail!("encrypted record too large: {} bytes", ciphertext.len());
    }
    w.write_all(&(ciphertext.len() as u32).to_be_bytes())?;
    w.write_all(ciphertext)?;
    w.flush()?;
    Ok(())
}

/// Read one encrypted record.
pub fn read_record<R: Read>(r: &mut R) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).context("read record length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_RECORD {
        bail!("record exceeds maximum: {len} bytes");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).context("read record body")?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

/// Split a logical message body into plaintext chunks ready for Noise encryption.
pub fn chunk_message(msg_type: MsgType, msg_id: u32, body: &[u8]) -> Result<Vec<Vec<u8>>> {
    if body.len() > MAX_LOGICAL {
        bail!("logical message too large: {} bytes", body.len());
    }
    let max_data = MAX_CHUNK_PLAINTEXT - CHUNK_HEADER;
    let count = if body.is_empty() {
        1
    } else {
        (body.len() + max_data - 1) / max_data
    };
    if count > MAX_CHUNKS as usize {
        bail!("too many chunks: {count}");
    }

    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let begin = index * max_data;
        let end = (begin + max_data).min(body.len());
        let data = &body[begin..end];
        let mut chunk = Vec::with_capacity(CHUNK_HEADER + data.len());
        chunk.extend_from_slice(&msg_type.code().to_le_bytes());
        chunk.extend_from_slice(&msg_id.to_le_bytes());
        chunk.extend_from_slice(&(index as u16).to_le_bytes());
        chunk.extend_from_slice(&(count as u16).to_le_bytes());
        chunk.extend_from_slice(data);
        out.push(chunk);
    }
    Ok(out)
}

struct Partial {
    msg_type: MsgType,
    count: u16,
    next: u16,
    body: Vec<u8>,
    started: Instant,
}

/// Reassembles chunked logical messages.
#[derive(Default)]
pub struct Reassembler {
    partial: HashMap<u32, Partial>,
    total_bytes: usize,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push one decrypted chunk. Returns a complete message when available.
    pub fn push(&mut self, plaintext: &[u8]) -> Result<Option<(MsgType, Vec<u8>)>> {
        if plaintext.len() < CHUNK_HEADER {
            bail!("chunk shorter than header");
        }
        let ty_code = u16::from_le_bytes([plaintext[0], plaintext[1]]);
        let msg_type = MsgType::from_code(ty_code)
            .with_context(|| format!("unknown chunked message type 0x{ty_code:04x}"))?;
        let msg_id = u32::from_le_bytes([
            plaintext[2],
            plaintext[3],
            plaintext[4],
            plaintext[5],
        ]);
        let index = u16::from_le_bytes([plaintext[6], plaintext[7]]);
        let count = u16::from_le_bytes([plaintext[8], plaintext[9]]);
        let data = &plaintext[CHUNK_HEADER..];

        if count == 0 || count > MAX_CHUNKS {
            bail!("invalid chunk count {count}");
        }
        if index >= count {
            bail!("chunk index {index} >= count {count}");
        }

        let completed = {
            let entry = self.partial.entry(msg_id).or_insert_with(|| Partial {
                msg_type,
                count,
                next: 0,
                body: Vec::new(),
                started: Instant::now(),
            });
            if entry.msg_type != msg_type || entry.count != count {
                bail!("inconsistent chunk metadata for message {msg_id}");
            }
            if index < entry.next {
                // Duplicate/old chunk; ignore.
                return Ok(None);
            }
            if index > entry.next {
                bail!("chunk gap for message {msg_id}: expected {}, got {index}", entry.next);
            }
            entry.body.extend_from_slice(data);
            self.total_bytes += data.len();
            if entry.body.len() > MAX_LOGICAL || self.total_bytes > MAX_LOGICAL * 2 {
                bail!("reassembled message exceeds size limit");
            }
            entry.next += 1;
            if entry.next == entry.count {
                Some(())
            } else {
                None
            }
        };

        if completed.is_some() {
            let entry = self.partial.remove(&msg_id).expect("entry exists");
            self.total_bytes = self.total_bytes.saturating_sub(entry.body.len());
            Ok(Some((entry.msg_type, entry.body)))
        } else {
            Ok(None)
        }
    }

    /// Drop incomplete messages older than the timeout.
    pub fn cleanup(&mut self) {
        let now = Instant::now();
        let mut freed = 0usize;
        self.partial.retain(|_, p| {
            let keep = now.duration_since(p.started) <= PARTIAL_TIMEOUT;
            if !keep {
                freed += p.body.len();
            }
            keep
        });
        self.total_bytes = self.total_bytes.saturating_sub(freed);
    }

    #[cfg(test)]
    pub fn pending_messages(&self) -> usize {
        self.partial.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn plain_roundtrip() {
        let msg = Message::ClientHello(HelloBody::new("viewer"));
        let (ty, body) = (msg.msg_type(), msg.encode().unwrap());
        let mut buf = Vec::new();
        write_plain(&mut buf, ty, &body).unwrap();
        let (ty2, body2) = read_plain(&mut Cursor::new(buf)).unwrap();
        assert_eq!(ty, ty2);
        assert_eq!(body, body2);
    }

    #[test]
    fn chunk_roundtrip_small() {
        let body = vec![7u8; 1000];
        let chunks = chunk_message(MsgType::VideoFrame, 42, &body).unwrap();
        assert_eq!(chunks.len(), 1);
        let mut re = Reassembler::new();
        let out = re.push(&chunks[0]).unwrap().unwrap();
        assert_eq!(out.0, MsgType::VideoFrame);
        assert_eq!(out.1, body);
    }

    #[test]
    fn chunk_roundtrip_large() {
        let body: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let chunks = chunk_message(MsgType::VideoFrame, 7, &body).unwrap();
        assert!(chunks.len() > 1);
        let mut re = Reassembler::new();
        let mut out = None;
        for c in &chunks {
            if let Some(m) = re.push(c).unwrap() {
                out = Some(m);
            }
        }
        let (ty, got) = out.expect("complete");
        assert_eq!(ty, MsgType::VideoFrame);
        assert_eq!(got, body);
        assert_eq!(re.pending_messages(), 0);
    }

    #[test]
    fn chunk_empty_body() {
        let chunks = chunk_message(MsgType::RequestKeyframe, 1, &[]).unwrap();
        assert_eq!(chunks.len(), 1);
        let mut re = Reassembler::new();
        let (ty, body) = re.push(&chunks[0]).unwrap().unwrap();
        assert_eq!(ty, MsgType::RequestKeyframe);
        assert!(body.is_empty());
    }

    #[test]
    fn chunk_bad_count() {
        let mut chunk = vec![0u8; CHUNK_HEADER];
        chunk[0..2].copy_from_slice(&MsgType::Ping.code().to_le_bytes());
        chunk[8..10].copy_from_slice(&0u16.to_le_bytes());
        assert!(Reassembler::new().push(&chunk).is_err());
    }

    #[test]
    fn record_too_large() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_RECORD as u32 + 1).to_be_bytes());
        assert!(read_record(&mut Cursor::new(buf)).is_err());
    }

    #[test]
    fn decode_rejects_absurd_alloc_length() {
        // A VideoFrame body whose Vec length claims u64::MAX must fail cleanly
        // (bounded decode), not attempt a huge allocation.
        let mut body = Vec::new();
        body.extend_from_slice(&0u64.to_le_bytes()); // seq
        body.extend_from_slice(&0u64.to_le_bytes()); // pts_ms
        body.push(0); // keyframe
        body.extend_from_slice(&1u32.to_le_bytes()); // width
        body.extend_from_slice(&1u32.to_le_bytes()); // height
        body.extend_from_slice(&u64::MAX.to_le_bytes()); // data length
        assert!(Message::decode(MsgType::VideoFrame, &body).is_err());
    }

    #[test]
    fn message_schema_roundtrip() {
        let info = Message::SessionInfo(SessionInfo {
            session_id: 9,
            width: 1920,
            height: 1080,
            fps: 30,
            monitors: vec![MonitorInfo {
                id: 1,
                name: "DISPLAY1".into(),
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                primary: true,
                scale: 1.5,
            }],
            codec: Codec::Vp8,
        });
        let ty = info.msg_type();
        let body = info.encode().unwrap();
        let back = Message::decode(ty, &body).unwrap();
        match back {
            Message::SessionInfo(s) => {
                assert_eq!(s.width, 1920);
                assert_eq!(s.monitors[0].scale, 1.5);
            }
            _ => panic!("wrong variant"),
        }
    }
}
