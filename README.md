# EFI SMBIOS Spoofer — Rust Rewrite

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.80+-orange.svg)](https://rust-lang.org)
[![UEFI](https://img.shields.io/badge/UEFI-x86__64-green.svg)](https://uefi.org)

A **UEFI framebuffer application** that modifies SMBIOS tables in memory before Windows boots. Written in Rust, renders a GUI via GOP (Graphics Output Protocol). Port of [Acrozi/SmbiosChanger](https://github.com/Acrozi/SmbiosChanger).

<p align="center">
  <img src="https://raw.githubusercontent.com/7ShIkI3/smbios-spoofer-rs/main/images/spoof.png" alt="SMBIOS Spoofer GUI" width="700">
</p>

## Features

- **GOP framebuffer GUI** — Custom bitmap font rendering, no console text dependencies
- **Spoofs SMBIOS Types 1–4** — System UUID, serials for System/Baseboard/Chassis/Processor
- **Vendor-aware serial generation** — Detects Dell, Lenovo, HP, Apple, ASUS, MSI, Gigabyte, Acer and matches their serial formats
- **Double persistence** — NVRAM + EFI partition fallback with checksum validation
- **Headless mode** — `cargo build --features headless` for zero-output stealth boot (<100ms)
- **Auto-boot countdown** — Progress bar with interactive reset (R key)
- **24 KB binary** — Smaller than the original C version
- **Safe where possible** — `unsafe` blocks are minimal, documented, and justified

## Build

```bash
# Prerequisites
rustup target add x86_64-unknown-uefi
rustup component add rust-src

# Build (release)
cargo build --release

# Build (headless — no display)
cargo build --release --features headless

# Binary location
# target/x86_64-unknown-uefi/release/smbios-spoofer.efi
```

## Install

1. **Disable Secure Boot** in BIOS (or sign with `./sign.sh` + MOK enrollment)
2. **Copy the .efi** to your EFI System Partition:

```cmd
# Windows (admin terminal)
mountvol S: /S
copy target\x86_64-unknown-uefi\release\smbios-spoofer.efi S:\EFI\SmbiosSpoofer\SmbiosSpooferV3.efi
mountvol S: /D
```

3. **Add a UEFI boot entry** via EasyUEFI (Windows) or `efibootmgr` (Linux):
   - Type: "Linux or other OS"
   - Path: `\EFI\SmbiosSpoofer\SmbiosSpooferV3.efi`

4. **Set boot order** so the spoofer runs FIRST (before Windows Boot Manager)

5. **Reboot**

The spoofer runs before Windows, patches SMBIOS tables, then hands off to the Windows bootloader.

## Architecture

```
src/
├── main.rs         # Entry point, orchestration
├── gfx.rs           # GOP framebuffer GUI (replaces text console)
├── smbios.rs       # #[repr(C)] SMBIOS types, table navigation, EditString
├── finder.rs       # Discovery via Config Table + HOB List fallback
├── patch.rs        # In-memory patching (unsafe, documented)
├── persistence.rs  # NVRAM + EFI partition save/load (checksum)
└── utils.rs        # RNG, UUID v4, vendor-aware serial generation
```

### Runtime flow

1. `main()` → UEFI entry point
2. `finder::find_smbios_entry()` → locate SMBIOS tables
3. `persistence::load_config()` → NVRAM → disk fallback
4. If nothing saved → generate format-matched serials for detected vendor
5. `patch::patch_all()` → direct memory write (unsafe)
6. `persistence::save_config()` → persist for next boot
7. GUI renders before/after summary with progress bar
8. Auto-boot countdown or interactive reset (R key)

## Spooped indentifiers

| Identifier | SMBIOS Type | Method |
|---|---|---|
| System UUID | Type 1 | Direct 16-byte overwrite |
| System Serial | Type 1 | EditString (in-place) |
| Baseboard Serial | Type 2 | EditString (in-place) |
| Chassis Serial | Type 3 | EditString (in-place) |
| Processor Serial | Type 4 | EditString (in-place) |

**Not spoofed:** MAC addresses, disk serials, CPUID, GPU IDs.

## License

MIT — See [LICENSE](LICENSE).

## Credits

- Original C project: [Acrozi/SmbiosChanger](https://github.com/Acrozi/SmbiosChanger)
- UEFI crate: [rust-osdev/uefi-rs](https://github.com/rust-osdev/uefi-rs)
- 8×8 bitmap font: public domain
