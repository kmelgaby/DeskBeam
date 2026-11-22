# DeskBeam Milestones

Scope is ordered by the brief's priority. Each milestone is independently
buildable and testable. The MVP is M1.

## M0 — Skeleton and toolchain (complete)

- Cargo project, module boundaries, CI-friendly `cargo check`.
- Windows DPI awareness, CLI parsing, egui window shell.
- Prebuilt libvpx integration decision recorded.

**Exit:** `cargo build --release` produces `deskbeam.exe`.

## M1 — MVP: encrypted screen view + input (current)

### Delivered

- Host: GUI, local IPv4 listing, port selection, 9-digit one-time code,
  single-connection listener, Accept/Reject modal, active-session banner,
  statistics, Disconnect.
- Viewer: address + code entry, connect, waiting-for-accept state, VP8 stream
  display, pointer/keyboard forwarding, disconnect.
- SPAKE2 over the code; Noise transport; HKDF-derived PSK.
- GDI screen capture (primary monitor), BGRA→I420, libvpx VP8 encode/decode,
  adaptive CBR bitrate based on queue depth, encode time, and viewer stats.
- Input: normalized mouse movement, buttons, wheel, virtual-key keys, Unicode
  text; UIPI limitations surfaced in the UI.
- Ping/Pong RTT, keyframe requests, graceful `Bye`.
- Unit tests for framing/chunking/codec/coordinate mapping; loopback
  handshake test (correct and wrong code); `--selftest` end-to-end mode.

### Acceptance criteria

1. Two instances on one machine stream at 20–30 fps for 10 minutes without
   leaks or crashes.
2. Wrong code never establishes a session; code rotates; host shows a warning.
3. Host Accept is required; Reject closes the viewer with a clear message.
4. A packet capture shows no screen pixels, code, or message types in plaintext.
5. Mouse and keyboard operate the host from the viewer, including drag and
   modifier keys; text input works in Notepad.
6. Disconnect from either side returns both apps to a usable state.
7. Bitrate drops when the link is throttled and recovers when it clears.

### Verification (2026-11-22)

- `cargo test`: 19/19 passed (framing, chunking, SPAKE2+Noise, VP8 round trip,
  coordinate mapping, code formatting).
- `deskbeam --selftest`: PASS. Real 1920×1080 capture → I420 → VP8 → encrypted
  transport → decode → RGBA; host Accept enforced; wrong code rejected and code
  rotated.
- Two GUI instances, manual input injection, firewall/NAT, and soak tests have
  not yet been executed; see `test-results/0.1.0-2026-11-22.md`.

### Known MVP limits

- Primary monitor only; multi-monitor enumeration is present but selection is
  wired in M4.
- Clipboard, files, and chat are reserved in the protocol but not implemented.
- Windows only; Linux capture/input land in M6.
- No NAT traversal or relay (by design).

## M2 — Clipboard sync

- Text clipboard both directions, last-writer-wins with loop suppression.
- Cap 1 MiB; binary/image clipboard reserved.
- Toggle in both UIs; off by default.
- Rate limit (max 5 changes/s).
- **Acceptance:** copy in Notepad on either side appears on the other within
  300 ms; no infinite echo; disable stops sync immediately.

## M3 — File transfer

- Drag-and-drop from viewer onto the stream (and an "Upload file" button).
- Offer/Accept dialog on the host with size, name, and a destination folder.
- 64 KiB chunks, SHA-256 at the end, progress and cancel on both sides.
- Resume is out of scope; restart on failure with a clear message.
- **Acceptance:** 1 GB file transfers integrity-verified; cancel removes the
  partial file; progress is monotonic; host can reject.

## M4 — Multi-monitor selection

- Enumerate monitors with virtual-desktop geometry and DPI scale.
- Viewer dropdown switches the captured monitor; host may allow or deny.
- Coordinate mapping uses the selected monitor's rect and scale.
- Recreate capture/encoder on switch and force a keyframe.
- **Acceptance:** switching between 1080p/4K/portrait monitors keeps pointer
  alignment within 2 px at 100%/150%/200% scaling; no stale texture.

## M5 — Text chat

- Side panel, plain text, capped at 4 KiB/message, rate-limited.
- Delivered over the same encrypted transport; no persistence.
- **Acceptance:** bidirectional messages appear in order; disconnecting clears
  the panel; oversized/rapid messages are rejected gracefully.

## M6 — Hardening and Linux

- Idle timeout, per-IP handshake backoff, code expiry timer, session log
  (no secrets).
- Linux: PipeWire/Portal capture, XTest/uinput input, prebuilt libvpx for
  Ubuntu x86_64; AppImage or single static binary.
- Reproducible build script, signed release, SBOM.
- **Acceptance:** Linux↔Windows and Linux↔Linux sessions pass the M1 matrix;
  fuzzing of chunk reassembly finds no panic or unbounded allocation.

## Out of scope unless requested

- NAT traversal/relay, unattended access, file synchronization, audio,
  mobile clients, recording, multi-session concurrency, service mode,
  privilege elevation, clipboard images, printer redirection.
