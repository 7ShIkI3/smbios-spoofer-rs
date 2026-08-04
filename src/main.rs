//! EFI SMBIOS Spoofer — Version Rust
//!
//! Mode headless : `cargo build --release --features headless` → boot furtif <100ms.

#![no_std]
#![no_main]

extern crate alloc;

mod utils;
mod smbios;
mod finder;
mod patch;
mod persistence;
mod gfx;

use uefi::prelude::*;
use persistence::SpoofConfig;

// ─── Panic / Alloc ─────────────────────────────────────────────────────

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    gfx::fatal_error(&alloc::format!("{}", info));
    loop { uefi::boot::stall(core::time::Duration::from_secs(1)); }
}

#[cfg(not(test))]
#[global_allocator]
static GLOBAL_ALLOCATOR: uefi::allocator::Allocator = uefi::allocator::Allocator;

// ─── Contexte ──────────────────────────────────────────────────────────

struct SpooferContext {
    smbios_entry: Option<&'static smbios::SmbiosEntry>,
    config: SpoofConfig,
    orig_uuid: [u8; 16],
    orig_system_serial: [u16; 64],
    orig_baseboard_serial: [u16; 64],
    orig_baseboard_model: [u16; 64],
    orig_processor_serial: [u16; 64],
    orig_chassis_serial: [u16; 64],
    loaded_from_storage: bool,
}

impl SpooferContext {
    fn new() -> Self {
        Self {
            smbios_entry: None,
            config: SpoofConfig::default(),
            orig_uuid: [0u8; 16],
            orig_system_serial: [0u16; 64],
            orig_baseboard_serial: [0u16; 64],
            orig_baseboard_model: [0u16; 64],
            orig_processor_serial: [0u16; 64],
            orig_chassis_serial: [0u16; 64],
            loaded_from_storage: false,
        }
    }
}

// ─── Point d'entrée ────────────────────────────────────────────────────

#[entry]
fn main() -> Status {
    uefi::helpers::init().ok();
    gfx::print_banner();
    utils::random_init();

    let mut ctx = SpooferContext::new();

    // ─── Step 1 : Find SMBIOS ──────────────────────────────────────────
    gfx::print_step(1, "Searching SMBIOS tables");
    ctx.smbios_entry = finder::find_smbios_entry();
    match ctx.smbios_entry {
        Some(entry) => {
            gfx::print_step_ok();
            gfx::colored(&alloc::format!("  (SMBIOS v{}.{})\n", entry.major_version, entry.minor_version), uefi::proto::console::text::Color::LightGray);
        }
        None => {
            gfx::print_step_fail("Cannot locate SMBIOS tables");
            gfx::wait_any_key();
            return Status::NOT_FOUND;
        }
    }

    // ─── Step 2 : Load or generate ─────────────────────────────────────
    gfx::print_step(2, "Loading spoof profile");
    ctx.loaded_from_storage = false;
    if let Some(loaded) = persistence::load_config() {
        ctx.config = loaded;
        ctx.loaded_from_storage = true;
        gfx::print_step_ok();
        gfx::colored("  (Loaded from NVRAM/disk)\n", uefi::proto::console::text::Color::LightGray);
    } else {
        gfx::colored("not found\n", uefi::proto::console::text::Color::LightGray);
        gfx::colored("  → Generating new profile...\n", uefi::proto::console::text::Color::Cyan);
        let entry = ctx.smbios_entry.unwrap();
        unsafe {
            patch::read_originals(entry,
                &mut ctx.orig_uuid, &mut ctx.orig_system_serial,
                &mut ctx.orig_baseboard_serial, &mut ctx.orig_baseboard_model,
                &mut ctx.orig_processor_serial, &mut ctx.orig_chassis_serial);
        }
        ctx.config.generate_with_originals(
            &ctx.orig_system_serial, &ctx.orig_baseboard_serial,
            &ctx.orig_processor_serial, &ctx.orig_chassis_serial);
        gfx::print_step_ok();
    }

    // ─── Step 3 : Patch ────────────────────────────────────────────────
    gfx::print_step(3, "Patching SMBIOS tables");
    if let Some(entry) = ctx.smbios_entry {
        unsafe { patch::patch_all(entry, &ctx.config); }
        gfx::print_step_ok();
    } else {
        gfx::print_step_skipped();
    }

    // ─── Step 4 : Persist ──────────────────────────────────────────────
    gfx::print_step(4, "Persisting spoof profile");
    if !ctx.loaded_from_storage {
        persistence::save_config(&ctx.config);
        gfx::print_step_ok();
    } else {
        gfx::print_step_skipped();
    }

    // ─── Lire les originaux pour affichage ──────────────────────────────
    if ctx.loaded_from_storage {
        if let Some(entry) = ctx.smbios_entry {
            unsafe {
                patch::read_originals(entry,
                    &mut ctx.orig_uuid, &mut ctx.orig_system_serial,
                    &mut ctx.orig_baseboard_serial, &mut ctx.orig_baseboard_model,
                    &mut ctx.orig_processor_serial, &mut ctx.orig_chassis_serial);
            }
        }
    }

    // ─── Boucle interactive ────────────────────────────────────────────
    loop {
        gfx::print_summary(&ctx);
        match gfx::interactive_menu() {
            Some(true) => {
                // R → Reset
                ctx.config.generate_with_originals(
                    &ctx.orig_system_serial, &ctx.orig_baseboard_serial,
                    &ctx.orig_processor_serial, &ctx.orig_chassis_serial);
                if let Some(entry) = ctx.smbios_entry {
                    unsafe { patch::patch_all(entry, &ctx.config); }
                }
                persistence::save_config(&ctx.config);
            }
            _ => break, // Timeout ou autre touche → boot
        }
    }

    Status::SUCCESS
}
