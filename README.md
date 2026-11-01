# EFI SMBIOS Spoofer — Rust Bare-Metal Edition (`v0.2.0`)

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.85+-orange.svg)](https://rust-lang.org)
[![UEFI](https://img.shields.io/badge/UEFI-x86__64-green.svg)](https://uefi.org)

A **UEFI bare-metal application** written in Rust (`no_std`, `uefi 0.39`) that patches SMBIOS 2.x (32-bit `_SM_`) and SMBIOS 3.0+ (64-bit `_SM3_`) tables in memory before Windows or Linux boots, with a GOP framebuffer GUI, NVRAM + ESP persistence, and a **100% automatic installer** (`install.ps1` / `install.bat` — no EasyUEFI required).

<p align="center">
  <img src="https://raw.githubusercontent.com/7ShIkI3/smbios-spoofer-rs/main/images/spoof.png" alt="SMBIOS Spoofer GUI" width="700">
</p>

## Key Features (`v0.2.0`)

- **Full SMBIOS 2.x (`_SM_`) & SMBIOS 3.0+ (`_SM3_` 64-bit) support** — Patches both 32-bit and 64-bit tables if the firmware exposes both, and recalculates Entry Point checksums (`_SM_`, `_DMI_`, `_SM3_`).
- **Comprehensive SMBIOS Coverage (Types 0, 1, 2, 3, 4, 11, 17)**:
  - **Type 0 (BIOS)** — Vendor, Version (format-matched, e.g. `FX707ZM.315` → `FX707ZM.318`), Release Date
  - **Type 1 (System)** — UUID v4 (RFC 4122, encoded in SMBIOS 2.6+ mixed-endian wire format), Serial Number, SKU Number
  - **Type 2 (Baseboard)** — Motherboard Serial Number & Asset Tag
  - **Type 3 (Chassis)** — Enclosure Serial Number & Asset Tag
  - **Type 4 (Processor)** — CPU Serial Number & Asset Tag across all sockets
  - **Type 11 (OEM Strings)** — Deterministic per-profile OEM string replacement
  - **Type 17 (RAM Memory Devices)** — Deterministic per-slot DDR4/DDR5 DIMM Serial Number & Asset Tag spoofing
- **PI/DXE `EFI_SMBIOS_PROTOCOL` Sync** — Synchronizes string and UUID updates through the firmware's `EFI_SMBIOS_PROTOCOL` when available.
- **Safe In-Place String Table Editing** — Preserves exact string lengths and `\0\0` structure terminators (zero string-table truncation or corruption).
- **Vendor-Aware & Format-Matched Generation** — Detects Dell, Lenovo, HP, Apple, ASUS, MSI, Gigabyte, and ASRock formats, replaces OEM placeholders (`"To Be Filled By O.E.M."`, `"Default string"`, `"No Asset Tag"`), and preserves pure-hex serial patterns.
- **Multi-Volume Persistence (`v8`)** — Saves to UEFI NVRAM (`SmbiosSpoof`) and scans all `SimpleFileSystem` volumes to persist `\EFI\SmbiosSpoofer\uuid.dat` on the EFI System Partition (ESP), with automatic v3..v7 → v8 migration.
- **Automatic Windows Boot Manager Chainloading** — Automatically locates and starts `\EFI\Microsoft\Boot\bootmgfw.efi` with its volume `DevicePath` after spoofing.
- **100% Automatic Installer (`install.ps1` / `install.bat`)** — Installs Rust/UEFI target if missing, runs 121 unit tests, compiles the `.efi`, mounts the ESP, copies the binary, and registers the UEFI boot entry in `#1` position via `bcdedit` (`{fwbootmgr}`) without needing EasyUEFI.

## One-Click Automatic Installation (Windows)

1. **Disable Secure Boot** in your BIOS settings.
2. Double-click **`install.bat`** (or run PowerShell as Administrator):

```powershell
# Full automatic build + ESP deploy + UEFI BootOrder configuration
.\install.ps1

# Optional: enable MAC spoofing and/or stealth headless mode (<100ms boot)
.\install.ps1 -SpoofMac
.\install.ps1 -Headless -SpoofMac
```

3. **Reboot** your PC. The spoofer will run automatically before Windows and chainload Windows Boot Manager.
4. **Verify after reboot** (compares active Windows WMI/CIM SMBIOS values against your pre-spoof snapshot):

```powershell
.\install.ps1 -Mode status
```

### Other Installer Modes

```powershell
.\install.ps1 -Mode build-only      # Run 121 unit tests + build smbios-spoofer.efi only
.\install.ps1 -Mode install-only    # Deploy existing smbios-spoofer.efi to ESP + bcdedit
.\install.ps1 -Mode reset-profile   # Delete ESP uuid.dat to generate new IDs on next boot
.\install.ps1 -Mode uninstall       # Remove UEFI boot entry and clean up ESP files
```

## Manual Build & Tests

```bash
# Add UEFI target
rustup target add x86_64-unknown-uefi

# Run host unit & integration tests (121 tests)
cargo test --target x86_64-pc-windows-msvc

# Build release UEFI binary (GOP GUI mode)
cargo build --release --target x86_64-unknown-uefi

# Build with optional features
cargo build --release --target x86_64-unknown-uefi --features "spoof-mac"
cargo build --release --target x86_64-unknown-uefi --features "headless,spoof-mac"
```

## Architecture

```
src/
├── main.rs         # UEFI entry point, orchestration & Windows Boot Manager chainloader
├── gfx.rs          # GOP framebuffer GUI (RGBx/BGRx auto-detection, 8x8 bitmap font)
├── smbios.rs       # SMBIOS 2.x (_SM_) & 3.0 (_SM3_) structs, Type 0/1/2/3/4/11/17, EFI_SMBIOS_PROTOCOL
├── finder.rs       # Multi-layer discovery (UEFI Config Table _SM3_ + _SM_ & PI/DXE HOB List fallback)
├── patch.rs        # OriginalSmbiosData capture, in-place patching & Entry Point checksum recalculation
├── persistence.rs  # NVRAM + multi-volume ESP persistence (\EFI\SmbiosSpoofer\uuid.dat, v8 schema)
├── spoof_net.rs    # Optional MAC spoofing (--features spoof-mac) via SNP + NVRAM variables
└── utils.rs        # Hardware TSC + RTC RNG, RFC 4122 UUID (mixed-endian), vendor/RAM serial generators
```

## Spoofed Identifiers

| Identifier | SMBIOS Type | Method |
|---|---|---|
| BIOS Vendor / Version / Date | Type 0 | Format-matched in-place `edit_string` |
| System UUID | Type 1 | 16-byte SMBIOS 2.6+ mixed-endian overwrite + `EFI_SMBIOS_PROTOCOL` |
| System Serial & SKU Number | Type 1 | Vendor-aware / format-matched `edit_string` |
| Baseboard Serial & Asset Tag | Type 2 | Vendor-aware / format-matched `edit_string` |
| Chassis Serial & Asset Tag | Type 3 | Vendor-aware / format-matched `edit_string` |
| Processor Serial & Asset Tag | Type 4 | Format-matched `edit_string` (all CPU sockets) |
| OEM Strings | Type 11 | Deterministic per-profile obfuscation |
| RAM DIMM Serial & Asset Tag | Type 17 | Deterministic per-slot hex/format-matched `edit_string` |
| Network MAC Address (opt-in) | UEFI SNP / NVRAM | `--features spoof-mac` |

## License

MIT — See [LICENSE](LICENSE).
