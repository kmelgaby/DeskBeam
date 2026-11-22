//! Session cryptography: SPAKE2 over the one-time code, HKDF, and a Noise
//! transport with chunked logical messages.

use anyhow::{anyhow, bail, Context, Result};
use hkdf::Hkdf;
use sha2::Sha256;
use snow::{params::NoiseParams, Builder, TransportState};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::proto::{self, HelloBody, Message, MsgType, Reassembler, MAGIC, PROTOCOL_MAJOR};

const ID_VIEWER: &[u8] = b"deskbeam-viewer";
const ID_HOST: &[u8] = b"deskbeam-host";
const NOISE_PATTERN: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
const PSK_INFO: &[u8] = b"deskbeam/v1/noise-psk";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Shared Noise transport. `write_message` and `read_message` touch different
/// cipher states and nonce counters, so a short mutex is sufficient.
pub type SharedTransport = Arc<Mutex<TransportState>>;

pub fn shared(transport: TransportState) -> SharedTransport {
    Arc::new(Mutex::new(transport))
}

/// Keep only the digits of a user-entered code.
pub fn normalize_code(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn derive_psk(spake_secret: &[u8]) -> Result<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(None, spake_secret);
    let mut psk = [0u8; 32];
    hk.expand(PSK_INFO, &mut psk)
        .map_err(|_| anyhow!("HKDF output length invalid"))?;
    Ok(psk)
}

fn noise_builder<'a>(psk: &'a [u8; 32]) -> Result<Builder<'a>> {
    let params: NoiseParams = NOISE_PATTERN
        .parse()
        .map_err(|e| anyhow!("invalid Noise pattern: {e}"))?;
    Builder::new(params)
        .psk(0, psk)
        .map_err(|e| anyhow!("Noise PSK setup failed: {e}"))
}

fn check_hello(hello: &HelloBody) -> Result<()> {
    if hello.magic != MAGIC {
        bail!("bad magic");
    }
    if hello.major != PROTOCOL_MAJOR {
        bail!(
            "protocol major mismatch: peer {}, local {}",
            hello.major,
            PROTOCOL_MAJOR
        );
    }
    if hello.peer_label.len() > 64 {
        bail!("peer label too long");
    }
    Ok(())
}

fn send_message_plain<W: Write>(w: &mut W, message: &Message) -> Result<()> {
    let ty = message.msg_type();
    let body = message.encode()?;
    proto::write_plain(w, ty, &body)
}

fn recv_message_plain<R: Read>(r: &mut R) -> Result<Message> {
    let (ty, body) = proto::read_plain(r)?;
    Message::decode(ty, &body)
}

fn set_handshake_timeouts(stream: &TcpStream) -> Result<()> {
    stream.set_nodelay(true).ok();
    stream
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("set handshake read timeout")?;
    stream
        .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("set handshake write timeout")?;
    Ok(())
}

fn clear_handshake_timeouts(stream: &TcpStream) {
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(None);
}

/// Viewer side: version exchange, SPAKE2, Noise handshake.
pub fn client_handshake(stream: &mut TcpStream, code: &str) -> Result<TransportState> {
    set_handshake_timeouts(stream)?;

    let hello = HelloBody::new(format!("viewer {}", env!("CARGO_PKG_VERSION")));
    send_message_plain(stream, &Message::ClientHello(hello))?;

    match recv_message_plain(stream)? {
        Message::ServerHello(h) => check_hello(&h)?,
        Message::Close(c) => bail!("host closed: {} (code {})", c.reason, c.code),
        other => bail!("unexpected handshake message: {:?}", other.msg_type()),
    }

    let normalized = normalize_code(code);
    if normalized.len() != 9 {
        bail!("access code must contain 9 digits");
    }

    let password = Password::new(normalized.as_bytes());
    let id_a = Identity::new(ID_VIEWER);
    let id_b = Identity::new(ID_HOST);
    let (state, msg_a) = Spake2::<Ed25519Group>::start_a(&password, &id_a, &id_b);
    proto::write_plain(stream, MsgType::SpakeA, &msg_a)?;

    let (ty, msg_b) = proto::read_plain(stream)?;
    if ty != MsgType::SpakeB {
        bail!("expected SPAKE2 message B, got {ty:?}");
    }
    let key = state
        .finish(&msg_b)
        .map_err(|e| anyhow!("SPAKE2 confirmation failed (wrong code?): {e:?}"))?;
    let psk = derive_psk(&key)?;

    let mut noise = noise_builder(&psk)?.build_initiator()?;
    let mut buf = vec![0u8; proto::MAX_CHUNK_PLAINTEXT];
    let n = noise
        .write_message(&[], &mut buf)
        .map_err(|e| anyhow!("Noise write failed: {e}"))?;
    proto::write_plain(stream, MsgType::NoiseHandshake, &buf[..n])?;

    let (ty, msg) = proto::read_plain(stream)?;
    if ty != MsgType::NoiseHandshake {
        bail!("expected Noise handshake message, got {ty:?}");
    }
    noise
        .read_message(&msg, &mut buf)
        .map_err(|e| anyhow!("Noise handshake failed (wrong code?): {e}"))?;

    clear_handshake_timeouts(stream);
    noise
        .into_transport_mode()
        .map_err(|e| anyhow!("Noise transport setup failed: {e}"))
}

/// Host side: version exchange, SPAKE2, Noise handshake.
pub fn server_handshake(stream: &mut TcpStream, code: &str) -> Result<TransportState> {
    set_handshake_timeouts(stream)?;

    let hello = match recv_message_plain(stream)? {
        Message::ClientHello(h) => h,
        other => bail!("expected ClientHello, got {:?}", other.msg_type()),
    };
    if let Err(e) = check_hello(&hello) {
        let close = Message::Close(proto::CloseBody {
            code: 1,
            reason: "version_mismatch".to_string(),
        });
        let _ = send_message_plain(stream, &close);
        return Err(e);
    }
    let server_hello = HelloBody::new(format!("host {}", env!("CARGO_PKG_VERSION")));
    send_message_plain(stream, &Message::ServerHello(server_hello))?;

    let normalized = normalize_code(code);
    let password = Password::new(normalized.as_bytes());
    let id_a = Identity::new(ID_VIEWER);
    let id_b = Identity::new(ID_HOST);
    let (state, msg_b) = Spake2::<Ed25519Group>::start_b(&password, &id_a, &id_b);

    let (ty, msg_a) = proto::read_plain(stream)?;
    if ty != MsgType::SpakeA {
        bail!("expected SPAKE2 message A, got {ty:?}");
    }
    let key = state
        .finish(&msg_a)
        .map_err(|e| anyhow!("SPAKE2 confirmation failed (wrong code?): {e:?}"))?;
    let psk = derive_psk(&key)?;
    proto::write_plain(stream, MsgType::SpakeB, &msg_b)?;

    let mut noise = noise_builder(&psk)?.build_responder()?;
    let mut buf = vec![0u8; proto::MAX_CHUNK_PLAINTEXT];

    let (ty, msg) = proto::read_plain(stream)?;
    if ty != MsgType::NoiseHandshake {
        bail!("expected Noise handshake message, got {ty:?}");
    }
    noise
        .read_message(&msg, &mut buf)
        .map_err(|e| anyhow!("Noise handshake failed (wrong code?): {e}"))?;
    let n = noise
        .write_message(&[], &mut buf)
        .map_err(|e| anyhow!("Noise write failed: {e}"))?;
    proto::write_plain(stream, MsgType::NoiseHandshake, &buf[..n])?;

    clear_handshake_timeouts(stream);
    noise
        .into_transport_mode()
        .map_err(|e| anyhow!("Noise transport setup failed: {e}"))
}

/// Encrypt and send one logical message as one or more records.
/// `msg_id` is incremented once per logical message.
pub fn encrypt_write<W: Write>(
    transport: &SharedTransport,
    writer: &mut W,
    msg_type: MsgType,
    body: &[u8],
    msg_id: &mut u32,
) -> Result<()> {
    let chunks = proto::chunk_message(msg_type, *msg_id, body)?;
    *msg_id = msg_id.wrapping_add(1);

    let mut ciphertext = vec![0u8; proto::MAX_CHUNK_PLAINTEXT + 32];
    for chunk in chunks {
        let n = {
            let mut ts = transport
                .lock()
                .map_err(|_| anyhow!("transport lock poisoned"))?;
            ts.write_message(&chunk, &mut ciphertext)
                .map_err(|e| anyhow!("Noise encrypt failed: {e}"))?
        };
        proto::write_record(writer, &ciphertext[..n])?;
    }
    Ok(())
}

/// Read records until one complete logical message is reassembled.
pub fn decrypt_read<R: Read>(
    transport: &SharedTransport,
    reader: &mut R,
    reassembler: &mut Reassembler,
) -> Result<(MsgType, Vec<u8>)> {
    let mut plaintext = vec![0u8; proto::MAX_CHUNK_PLAINTEXT];
    loop {
        let record = proto::read_record(reader)?;
        let n = {
            let mut ts = transport
                .lock()
                .map_err(|_| anyhow!("transport lock poisoned"))?;
            ts.read_message(&record, &mut plaintext)
                .map_err(|e| anyhow!("Noise decrypt failed: {e}"))?
        };
        reassembler.cleanup();
        if let Some(message) = reassembler.push(&plaintext[..n])? {
            return Ok(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn code_normalization() {
        assert_eq!(normalize_code("123-456-789"), "123456789");
        assert_eq!(normalize_code(" 123 456 789 "), "123456789");
        assert_eq!(normalize_code("abc"), "");
    }

    #[test]
    fn psk_deterministic() {
        let a = derive_psk(b"secret").unwrap();
        let b = derive_psk(b"secret").unwrap();
        let c = derive_psk(b"other").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    fn roundtrip(code_host: &str, code_viewer: &str) -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host_code = code_host.to_string();

        let server = thread::spawn(move || -> Result<()> {
            let (mut stream, _) = listener.accept()?;
            let ts = server_handshake(&mut stream, &host_code)?;
            let shared = shared(ts);
            let mut reassembler = Reassembler::new();
            let (ty, body) = decrypt_read(&shared, &mut stream, &mut reassembler)?;
            let msg = Message::decode(ty, &body)?;
            match msg {
                Message::Ping(p) => {
                    let mut id = 1;
                    encrypt_write(
                        &shared,
                        &mut stream,
                        MsgType::Pong,
                        &Message::Pong(p).encode()?,
                        &mut id,
                    )?;
                    Ok(())
                }
                other => bail!("unexpected message {:?}", other.msg_type()),
            }
        });

        let client = (|| -> Result<()> {
            let mut stream = TcpStream::connect(addr)?;
            let ts = client_handshake(&mut stream, code_viewer)?;
            let shared = shared(ts);
            let mut id = 0;
            encrypt_write(
                &shared,
                &mut stream,
                MsgType::Ping,
                &Message::Ping(proto::PingBody {
                    nonce: 7,
                    sent_ms: 0,
                })
                .encode()?,
                &mut id,
            )?;
            let mut reassembler = Reassembler::new();
            let (ty, body) = decrypt_read(&shared, &mut stream, &mut reassembler)?;
            assert_eq!(ty, MsgType::Pong);
            match Message::decode(ty, &body)? {
                Message::Pong(p) => assert_eq!(p.nonce, 7),
                _ => bail!("wrong reply"),
            }
            Ok(())
        })();

        let server_result = server
            .join()
            .map_err(|_| anyhow!("server thread panicked"))?;
        client.context("client")?;
        server_result.context("server")?;
        Ok(())
    }

    #[test]
    fn handshake_and_transport_roundtrip() {
        roundtrip("123456789", "123456789").expect("handshake and Ping/Pong");
    }

    #[test]
    fn wrong_code_fails() {
        assert!(roundtrip("123456789", "987654321").is_err());
    }
}
