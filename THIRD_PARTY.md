# Third-Party Components

DeskBeam source is dual-licensed **MIT OR Apache-2.0**. All runtime components
use permissive or file-level-compatible licenses. No GPL, AGPL, SSPL, or
proprietary runtime components are used.

| Component | Version | License | Role |
|---|---|---|---|
| libvpx | 1.17.x (prebuilt) | BSD-3-Clause + patent grant | VP8/VP9 codec |
| shiguredo_libvpx | 2026.2.0-canary.3 | Apache-2.0 | Safe Rust libvpx bindings |
| egui / eframe | 0.37.x | MIT OR Apache-2.0 | GUI |
| snow | 0.10.x | Apache-2.0 OR MIT | Noise protocol |
| spake2 | 0.4.x | Apache-2.0 OR MIT | PAKE |
| hkdf | 0.13.x | MIT OR Apache-2.0 | Key derivation |
| sha2 | 0.11.x | MIT OR Apache-2.0 | Hash |
| rand | 0.10.x | MIT OR Apache-2.0 | CSPRNG |
| serde | 1.x | MIT OR Apache-2.0 | Serialization |
| bincode | 1.3.x | MIT | Wire encoding |
| crossbeam-channel | 0.5.x | MIT OR Apache-2.0 | Thread channels |
| anyhow / thiserror | 1.x / 2.x | MIT OR Apache-2.0 | Error handling |
| local-ip-address | 0.6.x | MIT OR Apache-2.0 | Address discovery |
| windows | 0.62.x | MIT OR Apache-2.0 | Win32 API |
| winit (via eframe) | — | Apache-2.0 | Window/event loop |
| curve25519-dalek (via spake2) | 4.x | BSD-3-Clause | Ed25519 group arithmetic |
| ring (via snow default resolver) | 0.17.x | ISC-style | AES-GCM/ChaCha20 primitives |

## Notes

- **libvpx** is BSD-licensed and royalty-free for VP8/VP9. The prebuilt archive
  is downloaded by `shiguredo_libvpx` at build time and verified by SHA-256.
- **shiguredo_libvpx** prefixes its libvpx symbols in prebuilt mode to avoid
  collisions; this does not change the license terms.
- **spake2** upstream notes it has not received an independent security audit.
  Treat it as a reviewed-but-unaudited primitive and re-evaluate before
  high-assurance deployment.
- **H.264/OpenH264** is deliberately not used: its library license is
  permissive, but MPEG-LA patent licensing would apply to distributed binaries.
- **Qt, MPL-licensed codec wrappers, and Electron** are not used.

Run `cargo metadata --format-version 1` and `cargo deny check licenses` on the
locked dependency graph to produce the authoritative list for a release.

## Distribution checklist

1. Include this file and the upstream license texts for libvpx and
   shiguredo_libvpx in the release package.
2. Include the MIT/Apache-2.0 notice for DeskBeam.
3. Record the exact libvpx archive SHA-256 in the release notes.
4. Do not add a dependency with a copyleft or proprietary license without
   updating this file and the project license decision.
