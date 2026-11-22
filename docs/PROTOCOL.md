# DeskBeam Protocol Specification

Version: **1.0** (wire major 1, minor 0)
Status: MVP implemented for types marked ✅; others reserved.

This document is normative for DeskBeam 1.x. Integers are **little-endian**
unless explicitly marked as the 4-byte record length, which is big-endian. All
lengths are byte counts.

## 1. Transport

- TCP, one connection per session, `TCP_NODELAY` enabled.
- Default port: **7878** (configurable on the host).
- The connection is bidirectional and symmetric after the handshake.
- No TLS record layer is used; confidentiality comes from Noise (section 3).

### 1.1 Record framing

Every record on the wire is:

```
+----------------+---------------------------+
| u32 BE length  | payload[length]           |
+----------------+---------------------------+
```

`length` counts the payload only and is capped at **65 535** for encrypted
records. Handshake records are capped at **4 096**. A record outside the cap is
a protocol error and the connection is closed.

## 2. Handshake

### 2.1 Overview

```
Viewer                                          Host
  |--- ClientHello (type 0x0001) ------------->|
  |<-- ServerHello (type 0x0002) --------------|
  |--- SPAKE2 A   (type 0x0010) -------------->|
  |<-- SPAKE2 B   (type 0x0011) ---------------|
  |         K = SPAKE2 secret                  |
  |         PSK = HKDF-SHA256(K)               |
  |--- Noise msg 1 (type 0x0020) ------------->|
  |<-- Noise msg 2 (type 0x0020) --------------|
  |=========== encrypted transport ============|
  |--- Hello       (type 0x0100, encrypted) -->|
  |<-- AcceptRequest (0x0101) ------------------|
  |         host user clicks Accept             |
  |<-- Accept (0x0102) + SessionInfo (0x0104) --|
  |<-- VideoFrame (0x0110) ... -----------------|
  |--- InputEvent (0x0120) ... --------------->|
```

Handshake deadline: **10 seconds** from accept to transport mode. One
authentication attempt per connection. On failure the host closes the socket,
rotates the one-time code, and surfaces a warning.

### 2.2 ClientHello / ServerHello

Plaintext body (after the u16 type):

```
magic:       [u8;4] = "DBM1"
major:       u16 = 1
minor:       u16 = 0
peer_label:  String   // informational, max 64 bytes ("viewer 0.1.0", hostname)
```

The server echoes its own version. If the client's major differs, the server
replies with `Close(0x00F0)` reason `version_mismatch` and closes. Minor
differences are accepted; unknown optional messages are ignored.

### 2.3 SPAKE2

- Implementation: SPAKE2 over the Ed25519 group (RustCrypto `spake2` 0.4).
- Password: the 9-digit code with separators removed, ASCII, no salt
  (SPAKE2 is not a password *hash*; the code is low-entropy by design and
  protected by the PAKE + one-guess-per-connection rule).
- Identities:
  - `id_a` (viewer) = `b"deskbeam-viewer"`
  - `id_b` (host)   = `b"deskbeam-host"`
- Messages A and B are opaque byte strings (33 bytes each in this group).
- Both sides derive `K` from `finish(peer_message)`. Any error aborts.

### 2.4 Noise handshake

```
pattern:  Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s
PSK:      HKDF-SHA256(ikm = K, salt = empty, info = "deskbeam/v1/noise-psk")
          output length 32 bytes
```

- Viewer is the initiator, host the responder.
- Message 1: viewer writes; host reads.
- Message 2: host writes; viewer reads.
- Both call `into_transport_mode()`. Any failure aborts with `auth_failed`.

The PSK binds the PAKE result to the Noise transcript. Ephemeral X25519 keys
provide forward secrecy. The host learns that the viewer knows the code; the
viewer learns that the host knows the code. Neither learns the code itself.

### 2.5 Post-handshake authorization

Authentication is **not** authorization. After transport mode:

1. Viewer sends `Hello` (0x0100) with its display name.
2. Host UI shows an Accept dialog with the peer address and display name.
3. Host sends `AcceptRequest` (0x0101) so the viewer can show
   "waiting for host".
4. On Accept, host sends `Accept` (0x0102) then `SessionInfo` (0x0104).
   On Reject, host sends `Reject` (0x0103) with a reason and closes.
5. No capture or input is processed before `Accept`.

## 3. Encrypted transport

### 3.1 Logical messages

After Noise transport mode, every logical message is:

```
msg_type:  u16 LE
msg_id:    u32 LE   // monotonically increasing, wraps; scoped to this session
body:      bytes    // bincode 1.3 fixint/little-endian serialization
```

Maximum logical message: **16 MiB**.

### 3.2 Chunking

Noise transport messages are capped at 65 535 bytes, so logical messages are
split. Each chunk plaintext is at most **60 000 bytes**:

```
msg_type:    u16 LE
msg_id:      u32 LE
chunk_index: u16 LE   // 0-based
chunk_count: u16 LE   // 1..=4096
data:        bytes    // 0..=(60000 - 10)
```

The chunk plaintext is encrypted with Noise, producing ciphertext
`len(data) + 10 + 16`. Each ciphertext is written as one record
(`u32 BE length || ciphertext`). There is no plaintext type byte outside the
ciphertext.

Receiver rules:

- Reassemble by `(msg_id)`; chunks must arrive in order (TCP), but the format
  permits reordering for a future datagram transport.
- Reject if `chunk_count == 0`, `chunk_count > 4096`, `chunk_index >= chunk_count`,
  or the accumulated size exceeds 16 MiB.
- Drop partial messages older than 5 seconds.
- Duplicate `msg_id` with different type/count is a protocol error.

### 3.3 bincode conventions

- Fixed-width integers, little-endian (`DefaultOptions::with_fixint_encoding`).
- `String`: `u64` byte length, then UTF-8.
- `Vec<T>`: `u64` element count, then elements.
- `Option<T>`: `u8` tag (0/1), then value.
- `enum`: `u32` variant index, then fields in declaration order.
- Struct fields are serialized in declaration order with no field names.

This is enough for an independent implementation to interoperate; the Rust
reference uses serde derive in the order shown below.

## 4. Message catalog

### 4.1 Plaintext handshake types

| ID | Name | Direction | Body |
|---|---|---|---|
| 0x0001 | ClientHello | V→H | `{ magic:"DBM1", major:u16, minor:u16, peer_label:String }` |
| 0x0002 | ServerHello | H→V | `{ magic:"DBM1", major:u16, minor:u16, peer_label:String }` |
| 0x0010 | SpakeA | V→H | opaque bytes |
| 0x0011 | SpakeB | H→V | opaque bytes |
| 0x0020 | NoiseHandshake | both | opaque bytes |
| 0x00F0 | Close | H→V | `{ code:u16, reason:String }` |
| 0x00F1 | AuthFailed | H→V | `{ code:u16 }` (best-effort, then close) |

Close codes: 1 version mismatch, 2 malformed handshake, 3 bad code,
4 timeout, 5 busy, 6 protocol error.

### 4.2 Encrypted control types

| ID | Name | Direction | Status | Body |
|---|---|---|---|---|
| 0x0100 | Hello | V→H | ✅ | `{ client_name:String, platform:String, capabilities:u32 }` |
| 0x0101 | AcceptRequest | H→V | ✅ | `{ session_id:u64 }` |
| 0x0102 | Accept | H→V | ✅ | `{ session_id:u64 }` |
| 0x0103 | Reject | H→V | ✅ | `{ reason:String }` |
| 0x0104 | SessionInfo | H→V | ✅ | `{ session_id:u64, width:u32, height:u32, fps:u16, monitors:[MonitorInfo], codec:Codec }` |
| 0x0110 | VideoFrame | H→V | ✅ | `{ seq:u64, pts_ms:u64, keyframe:bool, width:u32, height:u32, data:Vec<u8> }` |
| 0x0120 | InputEvent | V→H | ✅ | `{ event:InputEvent }` |
| 0x0121 | SelectMonitor | V→H | reserved | `{ id:u32 }` |
| 0x0130 | ClipboardText | both | reserved | `{ text:String }` |
| 0x0131 | ClipboardRequest | both | reserved | `{}` |
| 0x0140 | Ping | both | ✅ | `{ nonce:u64, sent_ms:u64 }` |
| 0x0141 | Pong | both | ✅ | `{ nonce:u64, sent_ms:u64 }` |
| 0x0150 | Stats | V→H | ✅ | `{ recv_fps:u16, kbps:u32, rtt_ms:u16, decode_ms:u16, want_keyframe:bool }` |
| 0x0151 | RequestKeyframe | V→H | ✅ | `{}` |
| 0x0160 | Bye | both | ✅ | `{ reason:String }` |
| 0x0170 | Chat | both | reserved | `{ text:String }` |
| 0x0180 | FileOffer | V→H | reserved | `{ id:u64, name:String, size:u64 }` |
| 0x0181 | FileAccept | H→V | reserved | `{ id:u64 }` |
| 0x0182 | FileChunk | V→H | reserved | `{ id:u64, offset:u64, data:Vec<u8> }` |
| 0x0183 | FileDone | V→H | reserved | `{ id:u64 }` |
| 0x0184 | FileCancel | both | reserved | `{ id:u64, reason:String }` |

### 4.3 Shared structures

```
MonitorInfo {
  id: u32,            // OS monitor handle (stable for the session)
  name: String,       // e.g. "\\\\.\\DISPLAY1"
  x: i32, y: i32,     // virtual-desktop position
  width: u32, height: u32,
  primary: bool,
  scale: f32          // reported DPI scale (1.0 = 96 dpi)
}

Codec { Vp8 = 0, Vp9 = 1 }   // MVP sends Vp8

InputEvent {
  MouseMove   { nx: u16, ny: u16 }        // 0..=65535, normalized to frame
  MouseButton { button: u8, down: bool }  // 0=L,1=R,2=M,3=X1,4=X2
  Wheel       { dx: i16, dy: i16 }
  Key         { vk: u16, down: bool, extended: bool }
  Text        { text: String }            // UTF-16 injection path
}
```

- `MouseMove` is normalized so the host can map it to the selected monitor
  regardless of scaling or letterboxing.
- `Key.vk` is a Windows virtual-key code. The host also receives `Text` for
  Unicode input, which uses `KEYEVENTF_UNICODE` and bypasses layout issues.
- `extended` sets `KEYEVENTF_EXTENDEDKEY` for arrows, Insert/Delete, Home/End,
  PageUp/PageDown, NumLock, and right-side modifiers.

## 5. Session lifecycle

1. **Listening** — host bound, code valid, no peer.
2. **Handshaking** — exactly one connection is processed at a time. Additional
   connections wait in the TCP listen backlog; they are handled only after the
   current session ends, by which point the one-time code has rotated, so they
   fail authentication. `Close(5 busy)` is reserved for a future concurrent
   accept loop.
3. **Authorizing** — host UI modal; no data flows except AcceptRequest/Ping.
4. **Active** — capture and input enabled; session banner visible.
5. **Closing** — `Bye` sent, sockets closed, threads joined or detached, code
   rotated, back to Listening if the listener is still enabled.

The viewer may disconnect at any time by closing the TCP socket; the host
detects EOF and ends the session, rotating the code.

## 6. Adaptive quality messages

The host adjusts its encoder using local signals (encode time, writer queue)
and viewer `Stats`. The viewer sends `Stats` once per second:

- `recv_fps`: frames actually displayed in the last second.
- `kbps`: received video kilobits per second.
- `rtt_ms`: latest Ping/Pong round trip, or 0 if unknown.
- `decode_ms`: average decode+convert time for the last second.
- `want_keyframe`: true after a decode error or a long gap.

The host replies to `RequestKeyframe` or `want_keyframe` by forcing a keyframe on
the next capture. Bitrate policy is in [ARCHITECTURE.md](ARCHITECTURE.md) §6.

## 7. Error handling

| Condition | Sender behavior | Receiver behavior |
|---|---|---|
| Record too large | — | Close with `protocol error` |
| Unknown message type | — | Close with `protocol error`; v1 has no capability negotiation |
| Malformed body | — | Close with `protocol error` |
| Chunk violation | — | Close; do not allocate beyond cap |
| Noise decrypt failure | — | Close; count as auth failure |
| Missing video sequence | Viewer may request keyframe | Host forces keyframe |
| Idle > 60 s (M6) | Both | `Bye("idle timeout")`, close |

## 8. Security notes for implementers

- Never send the code, a hash of the code, or a verifier on the wire.
- Bind the protocol version into the PSK info string if you change the
  handshake; do not silently reuse `deskbeam/v1/noise-psk` for v2.
- Treat the Accept gate as mandatory and unskippable. A CLI or automation flag
  that bypasses it changes the security model and must be documented.
- Rate-limit failed handshakes per source IP in the future; the MVP closes and
  rotates the code, which already prevents a single code from being guessed
  more than once per connection.
- `spake2` upstream states it has not received an independent audit. This is a
  known residual risk; review before deploying to hostile networks.
- The Noise nonce counters are per-direction and per-session. Never reuse a
  `TransportState` across sessions.

## 9. Worked byte example

A viewer sends `Hello` (type 0x0100, msg_id 0):

```
body = bincode({ client_name:"viewer", platform:"windows", capabilities:0 })
     = 06 00 00 00 00 00 00 00  "viewer"
       07 00 00 00 00 00 00 00  "windows"
       00 00 00 00

logical = 00 01            // type 0x0100 LE
          00 00 00 00      // msg_id 0
          <body>

chunk   = 00 01            // type
          00 00 00 00      // msg_id
          00 00            // chunk_index 0
          01 00            // chunk_count 1
          <logical after type/id>  // body only
```

The 10-byte chunk header is followed by the body; the type and id are repeated
per chunk so each chunk is independently dispatchable.
