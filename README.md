# DeskBeam

DeskBeam is an original, peer-to-peer remote desktop application: one portable
executable, no installer, no account, and **no central server**. One machine runs
as **host**, the other as **viewer**. The host shows its LAN/VPN address and a
one-time access code; the viewer enters both, the host clicks **Accept**, and an
end-to-end encrypted session starts.

> Status: **Milestone 1 (MVP) implemented and loopback-verified** — encrypted
> session establishment, host accept gate, VP8 screen streaming with adaptive
> bitrate, remote mouse/keyboard, and disconnect. Verified here: 19 unit tests
> and a real 1920×1080 capture→encode→encrypt→decode self-test, including
> wrong-code rejection. Manual two-machine, input-injection, NAT/firewall, and
> soak tests remain on the plan ([test results](test-results/0.1.0-2026-11-22.md)).
> Clipboard, file transfer, multi-monitor selection, and chat are specified in
> [docs/MILESTONES.md](docs/MILESTONES.md).

## Why this exists

- **Portable**: a single `deskbeam.exe`, no service, no background daemon.
- **Direct**: connects by `IP:port` over LAN or VPN. There is deliberately no
  relay, no hole punching, and no cloud dependency.
- **Consent-first**: the host must click Accept for every session. There is no
  unattended mode, no hidden access, and no stored credential.
- **Encrypted**: SPAKE2 over the one-time code, then a Noise protocol transport
  (X25519 + ChaCha20-Poly1305 + BLAKE2s). No plaintext leaves either machine.
- **Visible**: the host shows a persistent session-active banner, a red window
  title (`DeskBeam - SESSION ACTIVE`), peer address, live statistics, and a
  Disconnect button.

## Quick start (Windows)

```powershell
# Build (first time; see docs/BUILD.md for prerequisites)
cargo build --release
# or: .\build.ps1    (builds and copies to .\DeskBeam.exe)

# Host
.\target\release\deskbeam.exe --host

# Viewer (on the other PC)
.\target\release\deskbeam.exe --viewer 192.168.1.20:7878
```

Without arguments the app opens a GUI with Host and Viewer tabs. The host tab
shows every local IPv4 address, the listening port, and the current one-time
code. The viewer tab takes `IP:port` and the code.

The code is **single-use**. A failed authentication attempt, a rejected session,
or a completed session rotates it automatically.

## Feature status

| Priority | Feature | Status |
|---|---|---|
| a | Screen streaming (VP8, adaptive quality) | **Implemented; loopback verified** |
| b | Remote mouse and keyboard | **Implemented; mapping unit-tested, manual pending** |
| c | Clipboard sync | Planned (M2) |
| d | File transfer with progress | Planned (M3) |
| e | Multi-monitor selection | Planned (M4) |
| f | Text chat | Planned (M5) |

## Known limits (MVP)

- Primary monitor only; selection is milestone M4.
- Windows host only. The protocol/UI/viewer build on Linux, but capture and
  input backends land in M6.
- GDI capture does not include the mouse cursor and may show black for locked
  desktops or protected content; DXGI Desktop Duplication / Windows Graphics
  Capture is a later performance and compatibility upgrade.
- Numpad, extra mouse buttons, IME composition, and the Windows key are not
  mapped in M1. Ctrl/Alt/Shift shortcuts, text, drag, wheel, and extended keys
  are supported.
- No NAT traversal, relay, unattended access, audio, clipboard, files, chat,
  recording, or elevation. Those are explicit non-goals or later milestones.
- Endpoint-security products may block `SendInput`, GDI capture, or the loopback
  listener. Allow-list the binary under your organization's policy if needed.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) — modules, threading, data flow, threat model
- [Protocol specification](docs/PROTOCOL.md) — framing, handshake, message table
- [Milestones](docs/MILESTONES.md) — scope and acceptance criteria per milestone
- [Build instructions](docs/BUILD.md) — toolchain, single-exe packaging, Linux
- [Test plan](docs/TEST_PLAN.md) — unit, integration, security, NAT/firewall
- [Third-party licenses](THIRD_PARTY.md)
- [Reverse-engineering notes](docs/RE_NOTES.md) — what the bundled AnyDesk stub does and does not reveal

## Security summary

- **Authentication**: SPAKE2 (Ed25519 group) over the one-time 9-digit code.
  The code is never sent on the wire and cannot be attacked offline.
- **Transport**: `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s`, PSK derived from the
  SPAKE2 secret with HKDF-SHA-256. Forward secrecy comes from ephemeral X25519.
- **Authorization**: host Accept is a separate gate from authentication. A
  correct code alone does not start a session.
- **Abuse resistance**: one authentication attempt per connection; failures
  rotate the code; handshake and authorization timeouts; bounded message sizes
  and chunk reassembly limits. Idle timeout and per-IP backoff are M6.
- **Honest limits**: input injection cannot control elevated (UAC) windows unless
  the host runs elevated; Windows secure-desktop sequences (Ctrl+Alt+Del) cannot
  be injected; LAN/VPN reachability is the operator's responsibility. The
  optional `--code` command-line flag is visible to other processes; prefer the
  GUI or the code field on a shared machine.

`--selftest` is a local diagnostic: it binds an ephemeral loopback port,
auto-accepts one session, streams the real screen, and exits. It is not a
service mode and must not be left running as one.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full threat model.

## License

DeskBeam source is dual-licensed under MIT OR Apache-2.0. The bundled codec
(libvpx, BSD) and wrapper (shiguredo_libvpx, Apache-2.0) are compatible with
redistribution; see [THIRD_PARTY.md](THIRD_PARTY.md). No GPL or proprietary
components are used.
