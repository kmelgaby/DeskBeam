# Building DeskBeam

## 1. Windows (primary target)

### Prerequisites

| Component | Why | Verified here |
|---|---|---|
| Windows 10/11 x64 | Target | Windows 11 26200 |
| Rust stable, MSVC host | Compiler | 1.99.0 `x86_64-pc-windows-msvc` (1.100.0 ICEs on the build machine) |
| Visual Studio 2022 with **Desktop development with C++** | MSVC linker + Windows SDK | VS Community 2022, SDK 10.0.26100 |
| `curl.exe` and `tar.exe` | The codec crate downloads its prebuilt libvpx | Bundled with Windows 10 1803+ |
| NASM, CMake, MSYS2 | **Not required** for the default build | — |

The default `shiguredo_libvpx` build downloads a prebuilt static libvpx
(BSD-licensed, wrapper Apache-2.0) from the crate's signed release and verifies
its SHA-256. No native compilation, NASM, or CMake is needed.

If your organization forbids build-time downloads, use the `source-build`
feature instead (section 6), or vendor the prebuilt archive.

### Install the toolchain

```powershell
winget install --id Rustlang.Rustup --exact
rustup default stable-x86_64-pc-windows-msvc
```

Visual Studio: install the **Desktop development with C++** workload and a
Windows 10/11 SDK. `link.exe` does not need to be on PATH; Cargo locates MSVC
through the registry/`vswhere`.

### Build

```powershell
cd E:\Vibe\anydesk
cargo build --release
```

First build: typically 5–15 minutes (egui is the bulk; a low-memory machine may
need the single-job workaround below). The codec archive adds a few seconds and
is cached in `target/`.

Output:

```
target\release\deskbeam.exe        # 12,153,856 bytes / 11.6 MiB on the verified build
```

Copy that one file to any Windows x64 machine. No installer, no service, no
registry writes, no configuration files.

### Verify

```powershell
.\target\release\deskbeam.exe --version
.\target\release\deskbeam.exe --selftest   # loopback handshake + codec check
```

`--selftest` auto-accepts a single loopback session and exits. Do not leave it
running as a service; the normal host always requires a click on Accept. In a
release (GUI-subsystem) build, the result is written to
`deskbeam-selftest.txt`; the process exit code is authoritative.

### Runtime dependencies

The Rust MSVC default links the Visual C++ runtime dynamically
(`vcruntime140.dll`, `vcruntime140_1.dll`). Windows 10/11 usually has it; a
minimal clean VM may not. Check on the target:

```powershell
Get-ChildItem C:\Windows\System32\vcruntime140*.dll
```

If missing, install the Microsoft Visual C++ 2015–2022 Redistributable, or build
with the static CRT (section 5) and re-test the libvpx link. Static CRT must be
consistent across Rust and libvpx; do not mix.

## 2. Single portable executable

Default release build is already a single `.exe`. To minimize it:

The checked-in profile is deliberately memory-friendly:

```toml
[profile.release]
opt-level = 1
lto = false
codegen-units = 16
strip = true
```

libvpx (the expensive part) is a prebuilt optimized C library, so Rust
optimization level mainly affects color conversion and UI code. On a machine
with ample RAM, switch to `opt-level = 3`, `lto = "thin"`,
`codegen-units = 1` for a smaller, faster binary.

Do **not** UPX-pack the executable: it triggers antivirus heuristics and gains
little.

The binary contains no assets other than the egui default fonts. There is no
per-user data directory; the one-time code is memory-only.

## 3. Running

```powershell
# GUI (default)
.\deskbeam.exe

# Host, GUI with a specific port
.\deskbeam.exe --host --port 7878

# Viewer, prefilled address and code
.\deskbeam.exe --viewer 192.168.1.20:7878 --code 123-456-789
```

Firewall: on first listen, Windows Defender Firewall prompts. Allow **Private**
networks for LAN use. For VPN, allow the VPN adapter's profile. DeskBeam does
not open firewall rules itself.

## 4. Linux (protocol/UI build now; capture/input in M6)

```bash
# Ubuntu 22.04+ / Debian 12+
sudo apt install build-essential pkg-config libxkbcommon-dev \
  libwayland-dev libx11-dev libxi-dev libxcursor-dev libxrandr-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo build --release
```

The protocol, UI, and codec compile on Linux. Screen capture and input are
implemented for Windows only in M1; the Linux backend (PipeWire/Portal +
XTest/uinput) is milestone M6. The same `deskbeam` binary connects to a Windows
host in viewer mode as soon as the platform module exists.

`shiguredo_libvpx` provides prebuilt libvpx for Ubuntu x86_64/arm64 and macOS
arm64; other distributions fall back to `--features source-build`.

## 5. Static CRT (optional)

`.cargo/config.toml`:

```toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

Caveats:

- The prebuilt libvpx was built against a specific CRT. If linking fails with
  duplicate/missing runtime symbols, use `shiguredo_libvpx` with
  `source-build`, which compiles libvpx with the selected CRT.
- Static CRT increases size and can interact with third-party DLLs. Test on a
  clean VM before distributing.

## 6. Building libvpx from source (air-gapped / policy)

```powershell
cargo build --release --features codec-source
```

This enables `shiguredo_libvpx/source-build`, which requires:

- Git (to clone libvpx v1.17.0)
- A POSIX shell (`sh`) and `make` (Git Bash or MSYS2)
- `nasm` on PATH
- `msbuild.exe` from Visual Studio

Install NASM portably:

```powershell
# Download the official win64 zip, extract to a tools directory, add to PATH
# https://www.nasm.us/pub/nasm/releasebuilds/
```

Source builds are slower and use more disk; the default prebuilt path is
preferred for normal development.

## 7. Reproducibility and supply chain

- `Cargo.lock` is committed; build release binaries from a clean checkout.
- `shiguredo_libvpx` downloads a versioned archive and verifies SHA-256. Pin the
  crate version and record the archive hash in your build log.
- Run `cargo deny check licenses advisories` before release.
- Generate an SBOM with `cargo cyclonedx` or `cargo sbom`.
- Sign the final `.exe` with an Authenticode certificate for distribution;
  unsigned remote-control binaries are frequently flagged by SmartScreen.
- The protocol has no auto-update mechanism. Distribution and patching are the
  operator's responsibility.

## 8. Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `link.exe not found` | C++ workload missing | Install Desktop development with C++ |
| `failed to download prebuilt library` | No GitHub access | Use `--features codec-source` or vendor the archive |
| Build error about `VPX` | Stale target dir after switching codec features | `cargo clean` |
| `memory allocation ... failed` or `STATUS_STACK_BUFFER_OVERRUN` | Low commit/RAM during a parallel build | `cargo build --release -j1`; the checked-in profile is already memory-friendly |
| `LNK4099: PDB 'vpxmd.pdb' was not found` | Prebuilt libvpx objects ship without their PDB | Benign; ignore or suppress linker warnings |
| `internal compiler error` / `can't find crate for std` | Broken toolchain build | Install another stable: `rustup toolchain install 1.99.0 && rustup override set 1.99.0` |
| OOM specifically while compiling `windows` | Its generated API surface is large | Add `[profile.release.package.windows] opt-level = 0` (only that crate drops optimization) |
| Linker runs out of memory | `link.exe` peak | Try `RUSTFLAGS="-C linker=rust-lld"` (Rust ships LLD) |
| App opens but no capture | Running in a session without a desktop (e.g., service) | Run interactively; no service mode by design |
| Viewer cannot connect | Firewall/VPN/NAT | See the NAT/firewall test plan |
| Input does nothing in an app | Target window is elevated | Run DeskBeam elevated on the host if policy allows |
