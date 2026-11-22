# Reverse-Engineering Notes: Bundled `AnyDesk.exe`

These notes document the static analysis that informed the original design. They
are deliberately limited to identification and architecture; no protocol logic,
keys, or code were extracted. The target is third-party software, analyzed for
interoperability/design understanding only.

## 1. Sample

| Property | Value |
|---|---|
| Path | `E:\Vibe\anydesk\AnyDesk.exe` |
| Size | 3,670,480 bytes |
| SHA-256 | `fb8fb89b5f56ce2acd9668021a470a18b7898808750800861151e908d5b1a20e` |
| Type | PE32 GUI, Intel i386, Windows 5.01 subsystem |
| Version | AnyDesk 6.0.8.0 (product 6.0), build timestamp 2020-09-01 |
| Company | philandro Software GmbH |
| Signature | Valid Authenticode, DigiCert, `CN=philandro Software GmbH` |

## 2. PE structure

| Section | Virtual size | Raw size | Entropy | Interpretation |
|---|---|---|---|---|
| `.text` | 0x2835 (~10 KB) | 0x2a00 | 6.515 | Loader/bootstrap only |
| `.itext` | 0x9f3600 | 0 | 0.000 | Virtual-only (no file backing) |
| `.rdata` | 0x2fa | 0x400 | 5.642 | Imports/debug |
| `.data` | 0x37623c (~3.6 MB) | 0x376000 | **8.000** | Encrypted/compressed payload |
| `.rsrc` | 0x3290 | 0x3400 | 6.684 | Manifest, version, icons |
| `.reloc` | 0x300 | 0x400 | 1.181 | Relocations |

- **No static imports and no exports.** Every API is resolved at runtime after
  the payload is unpacked.
- Entry point RVA `0x1ce9` in `.text`; `SizeOfImage` `0xd75000` despite a 10 KB
  code section — the loader maps and executes a large in-memory image.
- Debug directory points to
  `C:\Buildbot\ad-windows-32\build\release\app-32\win_loader\AnyDesk.pdb`.
- Overlay: 14,288 bytes after the last section (signature + certificate chain).

## 3. Manifest and resources

- `requestedExecutionLevel level="asInvoker"` — no elevation request.
- `dpiAware>true/PM` — per-monitor DPI awareness.
- Assembly identity `philandro.AnyDesk.AnyDesk`, x86, 6.0.8.0.
- Description: "AnyDesk screen sharing and remote control software."

## 4. String analysis

`strings` was not on PATH; strings were extracted with a PowerShell scanner into
`re_analysis/strings_ascii.txt` (6,062 ASCII strings ≥ 6 chars) and
`strings_utf16.txt` (18 UTF-16 strings).

Notable findings:

- Only loader/version/signature material is readable: DigiCert/OCSP/CRL URLs,
  the PDB path, the company name, and the manifest.
- No service URLs, relay endpoints, port numbers, protocol keywords, or session
  message strings appear in the clear.
- The overwhelming majority of the 3.6 MB `.data` region produces high-entropy
  garbage, consistent with an encrypted/compressed payload.

## 5. What this tells us (and what it does not)

**Evidence-backed conclusions:**

1. The distributed `AnyDesk.exe` is a **loader stub**. The real engine is
   embedded as an encrypted/compressed payload and loaded in memory.
2. It is a 32-bit x86 build, per-monitor DPI aware, and requests no elevation.
3. The binary is code-signed and carries a valid certificate chain.

**Not established (and intentionally not pursued):**

- The wire protocol, message formats, ports, and cryptographic design of
  AnyDesk. The payload is cryptographically protected; extracting it would be
  circumvention work with no bearing on an original design.
- Any code, algorithm, or asset reuse. None was performed.

## 6. Design implications adopted

| Observation | Original design decision |
|---|---|
| Tiny signed loader + self-contained payload | Ship one self-contained executable; no installer or service |
| `asInvoker` | Run at normal integrity; document UIPI limits instead of elevating |
| `dpiAware=true/PM` | Set per-monitor DPI awareness V2 at startup; normalize pointer coordinates |
| Encrypted payload hides protocol strings | Design and publish our own protocol instead of inferring theirs |
| Portable, no service | No persistence, no background daemon, no unattended mode |

## 7. Reproducing this analysis

```powershell
Get-Item AnyDesk.exe | Select-Object Length
Get-FileHash AnyDesk.exe -Algorithm SHA256
(Get-Item AnyDesk.exe).VersionInfo
Get-AuthenticodeSignature AnyDesk.exe
```

```powershell
# PE sections, entropy, imports (requires Python + pefile)
python -I re_analysis\tools\pe_info.py AnyDesk.exe
```

The scanner is saved at `re_analysis/tools/extract_strings.ps1`; the outputs are
in `re_analysis/`.

## 8. Scope statement

This analysis was performed on a local copy for architectural study. It did not
bypass licensing, DRM, authentication, or encryption. DeskBeam shares no code,
assets, protocol constants, or branding with AnyDesk.
