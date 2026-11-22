# DeskBeam Test Plan

Applies to milestone M1 unless marked otherwise. Record results per release in
`test-results/<version>.md`.

## 1. Environments

| ID | Host | Viewer | Network | Purpose |
|---|---|---|---|---|
| E1 | Windows 11 x64, 1080p | same machine | loopback | fast functional |
| E2 | Windows 11, 1080p 100% | Windows 10 22H2, 1080p 125% | 1 Gbps Ethernet | baseline LAN |
| E3 | Windows 11, 4K 150% | Windows 11, 1080p 100% | Wi-Fi 5 | scaling/perf |
| E4 | Windows 11 | Windows 11 | WireGuard VPN | VPN reachability |
| E5 | Windows 11 | Linux (viewer-only M1) | LAN | cross-platform protocol |
| E6 | Clean Windows 11 VM, no VC++ redist | Windows 11 | NAT | portability check |

## 2. Automated tests

Run:

```powershell
cargo test
.\target\release\deskbeam.exe --selftest
```

### 2.1 Unit

- `proto::framing`: round-trip lengths; reject > cap; partial reads.
- `proto::chunk`: 1-chunk and N-chunk round trips; out-of-order rejection;
  duplicate id; chunk_count 0; chunk_count > 4096; > 16 MiB total.
- `crypto`: SPAKE2 same code → equal keys; different code → mismatch; HKDF
  known-answer for a fixed input.
- `codec`: BGRA→I420 dimensions (odd sizes, 1×1, 4K); VP8 encode→decode PSNR
  > 30 dB on a synthetic gradient; keyframe flag; bitrate reconfigure.
- `input`: normalized coordinate mapping at 100%/150%/200% and letterboxed
  rects; clamp at edges; wheel sign.
- `util`: code formatting, normalization (`123-456-789` == `123456789`),
  rejection of non-digits; local IP filtering.

### 2.2 Integration

- `--selftest`: spawns a listener on `127.0.0.1:0`, connects a viewer, performs
  SPAKE2 + Noise, accepts through the real host accept gate, captures the real
  primary screen, encodes VP8, transports it encrypted, decodes it, and verifies
  the decoded 1920×1080 RGBA buffer. It then attempts a wrong-code connection
  and requires rejection. It does not inject input. Exits 0 on pass and writes
  `deskbeam-selftest.txt` (release builds hide the console).
- Loopback handshake test with wrong code: must fail within 2 s and not leak
  a session key.
- Two-process test (E1): host GUI + viewer GUI, manual accept, 10-minute soak.

## 3. Functional matrix

| # | Test | Expected |
|---|---|---|
| F1 | Host start | IP list, port, 9-digit code shown; status Listening |
| F2 | Viewer connect with correct code | Host shows Accept modal; viewer shows waiting |
| F3 | Host Accept | Session banner; stream within 2 s; input works |
| F4 | Host Reject | Viewer shows reason; host returns to Listening; code rotates |
| F5 | Wrong code | No Accept modal; host warning; code rotates; viewer error ≤ 3 s |
| F6 | Code reuse after session | Old code fails; new code works |
| F7 | Disconnect from host | Viewer returns to connect screen ≤ 1 s |
| F8 | Disconnect from viewer | Host returns to Listening; banner disappears |
| F9 | Host window close while active | Viewer detects EOF; host process exits cleanly |
| F10 | Bind port already in use | Actionable error; UI remains usable |
| F11 | Second viewer while active | Queued by the OS; cannot start until the active session ends, then fails because the code rotated; active session unaffected |
| F12 | Monitor sleep/lock | Session survives or fails clearly; no hang |

## 4. Input tests (Windows host)

| # | Test | Expected |
|---|---|---|
| I1 | Pointer mapping at 100% | Cursor lands within 2 px of the viewer's click |
| I2 | Pointer mapping at 125%/150%/200% | Same, per-monitor DPI aware |
| I3 | Drag a window | Press/move/release preserved; no stuck button |
| I4 | Right click, middle click | Context menu / autoscroll |
| I5 | Wheel up/down | Correct direction and magnitude |
| I6 | Modifiers (Ctrl/Shift/Alt/Win) | Chords work; release order respected |
| I7 | Extended keys (arrows, Home/End, Delete, NumLock) | Correct behavior |
| I8 | Unicode text in Notepad | Accented/CJK text appears |
| I9 | Elevated app (e.g., Task Manager as admin) | Input does not work; UI explains UIPI |
| I10 | Ctrl+Alt+Del | Not synthesizable; documented, not a failure |
| I11 | Host user moves local mouse during session | Local input is not blocked; remote input interleaves |

## 5. Security tests

| # | Test | Method | Expected |
|---|---|---|---|
| S1 | No plaintext | Wireshark `tcp.port==7878`; `strings` on the pcap | No code, screen text, or message type bytes; high entropy |
| S2 | Code never transmitted | Search pcap for `123-456-789` and `123456789` | Absent |
| S3 | Replay | Replay a recorded handshake to a fresh listener | Fails; code rotates |
| S4 | MITM without code | TCP proxy that forwards/modifies | Cannot derive keys; session aborts |
| S5 | Offline dictionary | Capture one failed handshake | No verifier recoverable |
| S6 | Online guessing | 1000 connections with random codes | Only one guess per connection; all fail; no lockout bypass |
| S7 | Accept bypass | Connect with correct code but never click Accept | No capture/input; only ping allowed |
| S8 | Reject authorization | Reject, then inspect viewer | No frame or input accepted after Reject |
| S9 | Oversized record | Send length 0xFFFFFFFF | Connection closed, no large allocation (measure RSS) |
| S10 | Chunk bomb | chunk_count 65535 / repeated ids | Closed at 4096 cap; RSS bounded |
| S11 | Malformed bincode | Random bodies for all types | No panic; close or ignore |
| S12 | Version downgrade | Client major 0 | Clean `version_mismatch`, no crypto attempted |
| S13 | Process inspection | Check disk, registry, temp | No code, key, or session data persisted |
| S14 | Session indicator | Accept a session | Banner always visible; no hidden mode |

Fuzz targets (M6): chunk reassembler, message decoder. Run `cargo fuzz` for at
least 10 minutes each.

## 6. Performance and soak

| # | Test | Expected |
|---|---|---|
| P1 | 1080p static desktop, 10 min | ≤ 300 kbps average; ≤ 5% CPU |
| P2 | 1080p scrolling/video | 2–8 Mbps; 20–30 fps; CPU ≤ 35% of a core |
| P3 | 4K capture | ≥ 15 fps at 8–12 Mbps, or clear degradation message |
| P4 | 8-hour soak | No memory growth > 5%, no handle leak, no disconnect |
| P5 | Bandwidth cap 1 Mbps (NetLimiter/clumsy) | Bitrate adapts within 5 s; no freeze |
| P6 | 2% packet loss / 100 ms jitter | TCP recovers; keyframes requested; no crash |
| P7 | Latency | Glass-to-glass ≤ 80 ms LAN at 1080p30 |
| P8 | Reconnect 100× | No port leak, no orphan threads |

Measure glass-to-glass with a millisecond clock displayed on the host and
photographed alongside the viewer screen; record in the release notes.

## 7. NAT and firewall test (required)

DeskBeam intentionally has **no relay and no hole punching**. The goal is to
verify honest failure and document what the operator must do.

### 7.1 Same LAN, firewall prompt allowed

1. Host starts listening. Accept the Windows Defender prompt for **Private**.
2. Viewer connects. Expected: works.
3. If blocked, add a rule:

```powershell
New-NetFirewallRule -DisplayName "DeskBeam 7878" -Direction Inbound `
  -Protocol TCP -LocalPort 7878 -Action Allow -Profile Private
```

### 7.2 Inbound blocked

```powershell
New-NetFirewallRule -DisplayName "DeskBeam BLOCK" -Direction Inbound `
  -Protocol TCP -LocalPort 7878 -Action Block
```

Expected: viewer fails with a connect timeout (not a handshake error), no
partial session, host remains Listening. Remove the rule afterwards.

Check reachability:

```powershell
Test-NetConnection 192.168.1.20 -Port 7878
```

### 7.3 Different subnets / router isolation

- Client isolation on Wi-Fi, or guest VLAN: connection times out. Document the
  required route/VLAN rule; confirm the app does not fall back to a relay.
- Port forward TCP 7878 on the router to the host, then connect to the public
  IP. Expected: works, but exposes the host; recommend VPN instead.
- Test with the firewall rule scoped to Public: traffic works only when the
  profile matches.

### 7.4 CGNAT / symmetric NAT

- Connect from outside without a port forward. Expected: timeout. This is the
  documented limitation.
- With WireGuard/Tailscale/ZeroTier: use the VPN address shown by the host;
  expected: works with no router changes.
- Verify the host lists the VPN adapter address (not just Wi-Fi/Ethernet).

### 7.5 VPN matrix

| VPN | Host address used | Expected |
|---|---|---|
| WireGuard | `10.x.x.x` | works |
| Tailscale | `100.x.x.x` | works |
| Corporate VPN with client isolation | VPN IP | times out; policy issue |

Capture packets on both ends to confirm direct traffic and no third-party
endpoint.

### 7.6 Port and MTU

- Change the host port (`--port 9000`) and verify the viewer uses it.
- Set MTU 1280 on the VPN; stream for 10 minutes. Expected: no stalls or
  repeated keyframes; TCP handles fragmentation at the IP layer.
- Test over a mobile hotspot with high jitter; bitrate should fall, not freeze.

## 8. Portability

| # | Test | Expected |
|---|---|---|
| T1 | Copy `deskbeam.exe` to a clean VM | Runs; prompts firewall on first listen |
| T2 | No VC++ redist on VM | Clear failure or static build; documented |
| T3 | Read-only USB stick | Runs; no writes attempted |
| T4 | Non-admin user | Runs; input limited to same-integrity windows |
| T5 | Different Windows display languages | UI renders; Unicode input works |
| T6 | Path with spaces/Unicode | Runs |

## 9. Exit criteria

M1 is releasable when:

- All automated tests pass on E1 and E2.
- F1–F12, I1–I11, S1–S14 pass; S9/S10 show bounded memory.
- P1–P4 and P7 meet or have a documented, accepted deviation.
- The NAT/firewall matrix is documented with screenshots of failure modes.
- No high-severity findings from `cargo audit` / `cargo deny`.
