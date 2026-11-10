//! EFI SMBIOS Spoofer — Version Rust (`v0.2.0`)
//!
//! Fonctionnalités :
//! - Support SMBIOS 2.x (32-bit `_SM_`) et SMBIOS 3.0+ (64-bit `_SM3_`)
//! - Spoofing Types 0 (BIOS), 1 (System UUID mixed-endian + Serial + SKU),
//!   2 (Baseboard Serial + AssetTag), 3 (Chassis Serial + AssetTag),
//!   4 (Processor Serial + AssetTag), 11 (OEM Strings) et 17 (RAM DIMM Serials)
//! - Recalcul des checksums d'Entry Point (`_SM_` / `_DMI_` / `_SM3_`) + sync `EFI_SMBIOS_PROTOCOL`
//! - Persistance NVRAM + partition ESP (`\EFI\SmbiosSpoofer\uuid.dat`) avec migration auto v3..v7 → v8
//! - Chainloading automatique de Windows Boot Manager (`\EFI\Microsoft\Boot\bootmgfw.efi`)

#![cfg_attr(target_os = "uefi", no_std)]
#![cfg_attr(target_os = "uefi", no_main)]

extern crate alloc;

pub mod finder;
pub mod gfx;
pub mod patch;
pub mod persistence;
pub mod smbios;
pub mod spoof_disk;
#[cfg(feature = "spoof-mac")]
pub mod spoof_net;
pub mod utils;

// Modules expérimentaux d'orchestration agentique (inclus en test ou via `--features agentic`)
#[cfg(any(feature = "agentic", test))]
pub mod agentic;
#[cfg(any(feature = "agentic", test))]
pub mod hooks;
#[cfg(any(feature = "agentic", test))]
pub mod memory;
#[cfg(any(feature = "agentic", test))]
pub mod reflection;
#[cfg(any(feature = "agentic", test))]
pub mod sandbox;

use patch::{OriginalSmbiosData, PatchReport};
use persistence::{PersistenceStatus, SpoofConfig};
use smbios::SmbiosTableInfo;
#[cfg(target_os = "uefi")]
use uefi::prelude::*;

// ─── Panic / Alloc (uniquement sur cible UEFI hors test) ───────────────

#[cfg(all(target_os = "uefi", not(test)))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    gfx::fatal_error(&alloc::format!("{}", info));
    loop {
        uefi::boot::stall(core::time::Duration::from_secs(1));
    }
}

#[cfg(all(target_os = "uefi", not(test)))]
#[global_allocator]
static GLOBAL_ALLOCATOR: uefi::allocator::Allocator = uefi::allocator::Allocator;

// ─── Contexte global d'exécution ───────────────────────────────────────

pub struct SpooferContext {
    pub smbios_info: Option<SmbiosTableInfo>,
    pub config: SpoofConfig,
    pub orig: OriginalSmbiosData,
    pub patch_report: PatchReport,
    pub persist_status: PersistenceStatus,
    pub disk_spoofed_count: u32,
    pub mac_spoofed_count: u32,
    pub nvram_mac_spoofed_count: u32,
    pub loaded_from_storage: bool,
}

impl Default for SpooferContext {
    fn default() -> Self {
        Self::new()
    }
}

impl SpooferContext {
    pub fn new() -> Self {
        Self {
            smbios_info: None,
            config: SpoofConfig::default(),
            orig: OriginalSmbiosData::default(),
            patch_report: PatchReport::default(),
            persist_status: PersistenceStatus::default(),
            disk_spoofed_count: 0,
            mac_spoofed_count: 0,
            nvram_mac_spoofed_count: 0,
            loaded_from_storage: false,
        }
    }
}

// ─── Étapes d'affichage ────────────────────────────────────────────────

#[cfg(target_os = "uefi")]
const TOTAL_STEPS: u8 = 4
    + if cfg!(feature = "spoof-disk") { 1 } else { 0 }
    + if cfg!(feature = "spoof-mac") { 1 } else { 0 };

// ─── Chainloader Windows Boot Manager ──────────────────────────────────

/// Chemins standards des chargeurs de démarrage Windows / Linux sur l'ESP.
#[cfg(target_os = "uefi")]
const WINDOWS_BOOTMGFW_PATH: &uefi::CStr16 = uefi::cstr16!("\\EFI\\Microsoft\\Boot\\bootmgfw.efi");

/// Recherche `\EFI\Microsoft\Boot\bootmgfw.efi` sur tous les volumes `SimpleFileSystem`
/// et le démarre avec son `DevicePath` pour que Windows trouve son magasin BCD.
/// Si Windows n'est pas trouvé ou si le chainload échoue, retourne silencieusement
/// afin que le Boot Manager UEFI prenne le relais via `BootOrder`.
#[cfg(target_os = "uefi")]
fn chainload_windows_bootmgr() {
    use alloc::vec;
    use uefi::Identify;
    use uefi::boot::{LoadImageSource, SearchType};
    use uefi::proto::device_path::DevicePath;
    use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode};
    use uefi::proto::media::fs::SimpleFileSystem;

    let Ok(handles) = uefi::boot::locate_handle_buffer(SearchType::ByProtocol(
        &SimpleFileSystem::GUID,
    )) else {
        return;
    };

    for &fs_handle in handles.iter() {
        // 1. Lire `\EFI\Microsoft\Boot\bootmgfw.efi` en mémoire depuis ce volume
        let file_bytes = {
            let Ok(mut fs) = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(fs_handle)
            else {
                continue;
            };
            let Ok(mut root) = fs.open_volume() else {
                continue;
            };
            let Ok(fh) = root.open(WINDOWS_BOOTMGFW_PATH, FileMode::Read, FileAttribute::empty())
            else {
                continue;
            };
            let Some(mut reg_file) = fh.into_regular_file() else {
                continue;
            };

            let Ok(info_box) = reg_file.get_boxed_info::<FileInfo>() else {
                continue;
            };
            let file_size = info_box.file_size() as usize;
            if file_size == 0 || file_size > 32 * 1024 * 1024 {
                continue;
            }

            let mut buf = vec![0u8; file_size];
            let Ok(read_len) = reg_file.read(&mut buf) else {
                continue;
            };
            buf.truncate(read_len);
            buf
        };

        if file_bytes.len() < 64 {
            continue;
        }

        // 2. Récupérer le DevicePath du volume ESP pour que bootmgfw.efi localise \EFI\Microsoft\Boot\BCD
        let dev_path_opt = uefi::boot::open_protocol_exclusive::<DevicePath>(fs_handle).ok();
        let load_source = match dev_path_opt.as_deref() {
            Some(dp) => LoadImageSource::FromBuffer {
                buffer: &file_bytes,
                file_path: Some(dp),
            },
            None => LoadImageSource::FromBuffer {
                buffer: &file_bytes,
                file_path: None,
            },
        };

        if let Ok(win_image_handle) = uefi::boot::load_image(uefi::boot::image_handle(), load_source)
        {
            // Libérer `dev_path_opt` avant de démarrer l'image Windows
            drop(dev_path_opt);
            let _ = uefi::boot::start_image(win_image_handle);
            return;
        }
    }
}

// ─── Point d'entrée UEFI ───────────────────────────────────────────────

#[cfg(target_os = "uefi")]
#[entry]
fn main() -> Status {
    let _ = uefi::helpers::init();
    gfx::print_banner();
    utils::random_init();

    let mut ctx = SpooferContext::new();
    let mut step_num: u8 = 1;

    // ─── Step 1 : Rechercher les tables SMBIOS (2.x et/ou 3.0 64-bit) ──
    gfx::print_step(step_num, TOTAL_STEPS, "Searching SMBIOS tables");
    ctx.smbios_info = finder::find_smbios_table();
    let info = match ctx.smbios_info {
        Some(info) => {
            gfx::print_step_ok();
            info
        }
        None => {
            gfx::print_step_fail("Cannot locate SMBIOS tables (_SM_ / _SM3_)");
            gfx::wait_any_key();
            return Status::NOT_FOUND;
        }
    };

    // IMPORTANT : Toujours lire les valeurs matérielles originales AVANT tout patch
    // afin que l'affichage "orig -> new" et la touche [R] utilisent les vrais originaux !
    ctx.orig = unsafe { patch::read_originals_info(&info) };
    #[cfg(feature = "spoof-disk")]
    {
        ctx.orig.disk_count = spoof_disk::read_original_disks(
            &mut ctx.orig.first_disk_serial,
            &mut ctx.orig.first_disk_model,
        );
    }

    // ─── Step 2 : Charger ou générer le profil de spoof ────────────────
    step_num += 1;
    gfx::print_step(step_num, TOTAL_STEPS, "Loading spoof profile");
    if let Some(mut loaded) = persistence::load_config() {
        if loaded.repair_with_originals(&ctx.orig) {
            let _ = persistence::save_config(&loaded);
        }
        ctx.config = loaded;
        ctx.loaded_from_storage = true;
        ctx.persist_status = PersistenceStatus {
            nvram_ok: true,
            disk_ok: true,
        };
        gfx::print_step_ok();
    } else {
        ctx.loaded_from_storage = false;
        ctx.config.generate_from_originals(&ctx.orig);
        gfx::print_step_ok();
    }

    // ─── Step 3 : Patcher toutes les tables SMBIOS (Types 0,1,2,3,4,11,17)
    step_num += 1;
    gfx::print_step(step_num, TOTAL_STEPS, "Patching SMBIOS + RAM tables");
    ctx.patch_report = unsafe { patch::patch_all_info(&info, &ctx.config) };
    gfx::print_step_ok();

    // ─── Step 4 : Spoof Disques NVMe / SATA / AHCI (`EFI_DISK_INFO_PROTOCOL`)
    #[cfg(feature = "spoof-disk")]
    {
        step_num += 1;
        gfx::print_step(step_num, TOTAL_STEPS, "Spoofing NVMe/SATA disk serials");
        ctx.disk_spoofed_count = spoof_disk::spoof_disks(&ctx.config);
        if ctx.disk_spoofed_count > 0 {
            gfx::print_step_ok();
        } else {
            gfx::print_step_skipped();
        }
    }

    // ─── Step 5 : Spoof MAC (opt-in via `--features spoof-mac`) ────────
    #[cfg(feature = "spoof-mac")]
    {
        step_num += 1;
        gfx::print_step(step_num, TOTAL_STEPS, "Spoofing MAC address");
        unsafe {
            ctx.mac_spoofed_count = spoof_net::spoof_mac_via_snp(&ctx.config);
            ctx.nvram_mac_spoofed_count = spoof_net::spoof_nvram_mac_addresses(&ctx.config);
        }
        if ctx.mac_spoofed_count + ctx.nvram_mac_spoofed_count > 0 {
            gfx::print_step_ok();
        } else {
            gfx::print_step_skipped();
        }
    }

    // ─── Step final : Sauvegarder en NVRAM + ESP Disk ──────────────────
    step_num += 1;
    gfx::print_step(step_num, TOTAL_STEPS, "Persisting spoof profile");
    if !ctx.loaded_from_storage {
        ctx.persist_status = persistence::save_config(&ctx.config);
        if ctx.persist_status.nvram_ok || ctx.persist_status.disk_ok {
            gfx::print_step_ok();
        } else {
            gfx::print_step_skipped();
        }
    } else {
        // S'assurer que le fichier ESP existe aussi si chargé depuis NVRAM uniquement
        let disk_ok = persistence::save_to_disk(&ctx.config).is_ok();
        ctx.persist_status.disk_ok = disk_ok;
        gfx::print_step_ok();
    }

    // ─── Boucle interactive (auto-boot après 5s ou touche immédiate) ───
    loop {
        gfx::print_summary(&ctx);
        match gfx::interactive_menu() {
            Some(true) => {
                // [R] → Régénérer un nouveau profil complet à partir des vrais originaux
                ctx.config.generate_from_originals(&ctx.orig);
                ctx.loaded_from_storage = false;
                ctx.patch_report = unsafe { patch::patch_all_info(&info, &ctx.config) };
                #[cfg(feature = "spoof-disk")]
                {
                    ctx.disk_spoofed_count = spoof_disk::spoof_disks(&ctx.config);
                }
                #[cfg(feature = "spoof-mac")]
                unsafe {
                    ctx.mac_spoofed_count = spoof_net::spoof_mac_via_snp(&ctx.config);
                    ctx.nvram_mac_spoofed_count =
                        spoof_net::spoof_nvram_mac_addresses(&ctx.config);
                }
                ctx.persist_status = persistence::save_config(&ctx.config);
            }
            _ => break,
        }
    }

    // Chainload automatique de Windows Boot Manager (`\EFI\Microsoft\Boot\bootmgfw.efi`)
    // Si le chainload direct ne prend pas la main, retourne Status::SUCCESS au Boot Manager UEFI.
    chainload_windows_bootmgr();

    Status::SUCCESS
}

/// Point d'entrée factice pour l'exécution des tests unitaires sur l'hôte (`cargo test`).
#[cfg(not(target_os = "uefi"))]
fn main() {}
