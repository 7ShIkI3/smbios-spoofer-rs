//! Module de rendu framebuffer GOP — interface graphique UEFI directe.
//!
//! Résout le bug hardware AMI Aptio : `SetAttribute()` est ignoré sur les
//! firmwares ASUS TUF/ROG. On dessine directement dans le framebuffer GOP.
//!
//! En mode headless (`--features headless`), toutes les fonctions sont no-op.

use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, Ordering};
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};
use uefi::proto::console::text::Color;

extern crate alloc;
use alloc::string::String;

// ─── État global du framebuffer ─────────────────────────────────────────

static mut GFX: Option<Gfx> = None;
static GFX_READY: AtomicBool = AtomicBool::new(false);

struct Gfx {
    fb: *mut u8,
    width: usize,
    height: usize,
    stride: usize,
    pixel_format: PixelFormat,
}

// ─── Couleurs (0x00RRGGBB en mémoire BGRx little-endian) ─────────────────

const C_BG: u32 = 0x00161B22; // fond sombre
const C_ACCENT: u32 = 0x00FFBF00; // ambre / or
const C_CYAN: u32 = 0x0000D4FF; // cyan
const C_OK: u32 = 0x002EA043; // vert
const C_FAIL: u32 = 0x00F85149; // rouge
const C_WHITE: u32 = 0x00F0F6FC;
const C_GRAY: u32 = 0x008B949E;
const C_DARK: u32 = 0x0030363D;
const C_PANEL: u32 = 0x0021262D;
const C_PROGRESS: u32 = 0x003FB950;
const C_BANNER_BG: u32 = 0x001F2937;

// ─── Layout ──────────────────────────────────────────────────────────────

const FONT_W: usize = 8;
const FONT_H: usize = 8;
const CHAR_SPACING: usize = 1;
const CHAR_STEP: usize = FONT_W + CHAR_SPACING;
const LINE_H: usize = FONT_H + 5;
const MARGIN: usize = 36;
const BANNER_H: usize = 72;
const BAR_H: usize = 54;
const PANEL_W: usize = 396;

// ─── Police bitmap 8×8 — ASCII 32-126 ────────────────────────────────────

const FONT_8X8: &[(u8, [u8; 8])] = &[
    (b' ', [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
    (b'!', [0x18, 0x3C, 0x3C, 0x18, 0x18, 0x00, 0x18, 0x00]),
    (b'"', [0x6C, 0x6C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
    (b'#', [0x6C, 0x6C, 0xFE, 0x6C, 0xFE, 0x6C, 0x6C, 0x00]),
    (b'$', [0x18, 0x3E, 0x60, 0x3C, 0x06, 0x7C, 0x18, 0x00]),
    (b'%', [0x00, 0x63, 0x66, 0x0C, 0x18, 0x33, 0x63, 0x00]),
    (b'&', [0x38, 0x6C, 0x38, 0x76, 0xDC, 0xCC, 0x76, 0x00]),
    (b'\'', [0x18, 0x18, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00]),
    (b'(', [0x0C, 0x18, 0x30, 0x30, 0x30, 0x18, 0x0C, 0x00]),
    (b')', [0x30, 0x18, 0x0C, 0x0C, 0x0C, 0x18, 0x30, 0x00]),
    (b'*', [0x00, 0x66, 0x3C, 0xFF, 0x3C, 0x66, 0x00, 0x00]),
    (b'+', [0x00, 0x18, 0x18, 0x7E, 0x18, 0x18, 0x00, 0x00]),
    (b',', [0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30, 0x00]),
    (b'-', [0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00]),
    (b'.', [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00]),
    (b'/', [0x06, 0x0C, 0x18, 0x30, 0x60, 0x40, 0x00, 0x00]),
    (b'0', [0x3C, 0x66, 0x6E, 0x76, 0x66, 0x66, 0x3C, 0x00]),
    (b'1', [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00]),
    (b'2', [0x3C, 0x66, 0x06, 0x0C, 0x30, 0x60, 0x7E, 0x00]),
    (b'3', [0x3C, 0x66, 0x06, 0x1C, 0x06, 0x66, 0x3C, 0x00]),
    (b'4', [0x06, 0x0E, 0x1E, 0x66, 0x7F, 0x06, 0x06, 0x00]),
    (b'5', [0x7E, 0x60, 0x7C, 0x06, 0x06, 0x66, 0x3C, 0x00]),
    (b'6', [0x3C, 0x66, 0x60, 0x7C, 0x66, 0x66, 0x3C, 0x00]),
    (b'7', [0x7E, 0x66, 0x0C, 0x18, 0x18, 0x18, 0x18, 0x00]),
    (b'8', [0x3C, 0x66, 0x66, 0x3C, 0x66, 0x66, 0x3C, 0x00]),
    (b'9', [0x3C, 0x66, 0x66, 0x3E, 0x06, 0x66, 0x3C, 0x00]),
    (b':', [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00]),
    (b';', [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x30, 0x00]),
    (b'<', [0x06, 0x0C, 0x18, 0x30, 0x18, 0x0C, 0x06, 0x00]),
    (b'=', [0x00, 0x00, 0x7E, 0x00, 0x7E, 0x00, 0x00, 0x00]),
    (b'>', [0x60, 0x30, 0x18, 0x0C, 0x18, 0x30, 0x60, 0x00]),
    (b'?', [0x3C, 0x66, 0x0C, 0x18, 0x18, 0x00, 0x18, 0x00]),
    (b'@', [0x3C, 0x66, 0x6E, 0x6A, 0x6E, 0x60, 0x3C, 0x00]),
    (b'A', [0x18, 0x3C, 0x66, 0x7E, 0x66, 0x66, 0x66, 0x00]),
    (b'B', [0x7C, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x7C, 0x00]),
    (b'C', [0x3C, 0x66, 0x60, 0x60, 0x60, 0x66, 0x3C, 0x00]),
    (b'D', [0x78, 0x6C, 0x66, 0x66, 0x66, 0x6C, 0x78, 0x00]),
    (b'E', [0x7E, 0x60, 0x60, 0x78, 0x60, 0x60, 0x7E, 0x00]),
    (b'F', [0x7E, 0x60, 0x60, 0x78, 0x60, 0x60, 0x60, 0x00]),
    (b'G', [0x3C, 0x66, 0x60, 0x6E, 0x66, 0x66, 0x3C, 0x00]),
    (b'H', [0x66, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x66, 0x00]),
    (b'I', [0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00]),
    (b'J', [0x1E, 0x0C, 0x0C, 0x0C, 0x0C, 0x6C, 0x38, 0x00]),
    (b'K', [0x66, 0x6C, 0x78, 0x70, 0x78, 0x6C, 0x66, 0x00]),
    (b'L', [0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x7E, 0x00]),
    (b'M', [0x63, 0x77, 0x7F, 0x6B, 0x63, 0x63, 0x63, 0x00]),
    (b'N', [0x66, 0x76, 0x7E, 0x7E, 0x6E, 0x66, 0x66, 0x00]),
    (b'O', [0x3C, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00]),
    (b'P', [0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60, 0x60, 0x00]),
    (b'Q', [0x3C, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x0E, 0x00]),
    (b'R', [0x7C, 0x66, 0x66, 0x7C, 0x78, 0x6C, 0x66, 0x00]),
    (b'S', [0x3C, 0x66, 0x60, 0x3C, 0x06, 0x66, 0x3C, 0x00]),
    (b'T', [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00]),
    (b'U', [0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00]),
    (b'V', [0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x18, 0x00]),
    (b'W', [0x63, 0x63, 0x63, 0x6B, 0x7F, 0x77, 0x63, 0x00]),
    (b'X', [0x66, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0x66, 0x00]),
    (b'Y', [0x66, 0x66, 0x66, 0x3C, 0x18, 0x18, 0x18, 0x00]),
    (b'Z', [0x7E, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x7E, 0x00]),
    (b'[', [0x3C, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3C, 0x00]),
    (b'\\', [0x60, 0x30, 0x18, 0x0C, 0x06, 0x02, 0x00, 0x00]),
    (b']', [0x3C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x3C, 0x00]),
    (b'^', [0x18, 0x3C, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00]),
    (b'_', [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7E, 0x00]),
    (b'`', [0x30, 0x18, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00]),
    (b'a', [0x00, 0x00, 0x3C, 0x06, 0x3E, 0x66, 0x3E, 0x00]),
    (b'b', [0x60, 0x60, 0x7C, 0x66, 0x66, 0x66, 0x7C, 0x00]),
    (b'c', [0x00, 0x00, 0x3C, 0x66, 0x60, 0x66, 0x3C, 0x00]),
    (b'd', [0x06, 0x06, 0x3E, 0x66, 0x66, 0x66, 0x3E, 0x00]),
    (b'e', [0x00, 0x00, 0x3C, 0x66, 0x7E, 0x60, 0x3C, 0x00]),
    (b'f', [0x0E, 0x18, 0x3E, 0x18, 0x18, 0x18, 0x18, 0x00]),
    (b'g', [0x00, 0x00, 0x3E, 0x66, 0x66, 0x3E, 0x06, 0x3C]),
    (b'h', [0x60, 0x60, 0x7C, 0x66, 0x66, 0x66, 0x66, 0x00]),
    (b'i', [0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x3C, 0x00]),
    (b'j', [0x06, 0x00, 0x06, 0x06, 0x06, 0x66, 0x3C, 0x00]),
    (b'k', [0x60, 0x60, 0x66, 0x6C, 0x78, 0x6C, 0x66, 0x00]),
    (b'l', [0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00]),
    (b'm', [0x00, 0x00, 0x63, 0x77, 0x7F, 0x6B, 0x63, 0x00]),
    (b'n', [0x00, 0x00, 0x7C, 0x66, 0x66, 0x66, 0x66, 0x00]),
    (b'o', [0x00, 0x00, 0x3C, 0x66, 0x66, 0x66, 0x3C, 0x00]),
    (b'p', [0x00, 0x00, 0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60]),
    (b'q', [0x00, 0x00, 0x3E, 0x66, 0x66, 0x3E, 0x06, 0x06]),
    (b'r', [0x00, 0x00, 0x7C, 0x66, 0x60, 0x60, 0x60, 0x00]),
    (b's', [0x00, 0x00, 0x3E, 0x60, 0x3C, 0x06, 0x7C, 0x00]),
    (b't', [0x18, 0x18, 0x7E, 0x18, 0x18, 0x18, 0x0E, 0x00]),
    (b'u', [0x00, 0x00, 0x66, 0x66, 0x66, 0x66, 0x3E, 0x00]),
    (b'v', [0x00, 0x00, 0x66, 0x66, 0x66, 0x3C, 0x18, 0x00]),
    (b'w', [0x00, 0x00, 0x63, 0x63, 0x6B, 0x7F, 0x36, 0x00]),
    (b'x', [0x00, 0x00, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0x00]),
    (b'y', [0x00, 0x00, 0x66, 0x66, 0x66, 0x3E, 0x06, 0x3C]),
    (b'z', [0x00, 0x00, 0x7E, 0x0C, 0x18, 0x30, 0x7E, 0x00]),
    (b'{', [0x0E, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0E, 0x00]),
    (b'|', [0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00]),
    (b'}', [0x70, 0x18, 0x18, 0x0E, 0x18, 0x18, 0x70, 0x00]),
    (b'~', [0x76, 0xDC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
];

// ─── Accès au Gfx ────────────────────────────────────────────────────────

fn init_gfx() {
    if GFX_READY.load(Ordering::Relaxed) {
        return;
    }

    let handle = match uefi::boot::get_handle_for_protocol::<GraphicsOutput>() {
        Ok(h) => h,
        Err(_) => return,
    };
    let mut gop = match uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle) {
        Ok(g) => g,
        Err(_) => return,
    };
    let info = gop.current_mode_info();

    // SAFETY: le framebuffer est alloué par le firmware et reste valide
    // jusqu'à l'exit des boot services. Environnement UEFI mono-thread.
    unsafe {
        GFX = Some(Gfx {
            fb: gop.frame_buffer().as_mut_ptr(),
            width: info.resolution().0,
            height: info.resolution().1,
            stride: info.stride(),
            pixel_format: info.pixel_format(),
        });
    }
    GFX_READY.store(true, Ordering::Relaxed);
}

fn get_gfx() -> Option<&'static mut Gfx> {
    if !GFX_READY.load(Ordering::Relaxed) {
        init_gfx();
    }
    let ptr: *mut Option<Gfx> = addr_of_mut!(GFX);
    unsafe { (*ptr).as_mut() }
}

fn with_gfx<F: FnOnce(&mut Gfx)>(f: F) {
    if cfg!(feature = "headless") {
        return;
    }
    if let Some(gfx) = get_gfx() {
        f(gfx);
    }
}

// ─── Impl Gfx ────────────────────────────────────────────────────────────

impl Gfx {
    /// Écrit un pixel en convertissant automatiquement `0x00RRGGBB` selon `self.pixel_format`.
    fn pixel(&mut self, x: usize, y: usize, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let out_color = match self.pixel_format {
            PixelFormat::Rgb => {
                let b = color & 0xFF;
                let g = color & 0xFF00;
                let r = (color >> 16) & 0xFF;
                (b << 16) | g | r
            }
            _ => color,
        };
        let offset = (y * self.stride + x) * 4;
        // SAFETY: `x < width` et `y < height`, `fb` est le pointeur GOP valide
        unsafe {
            let ptr = self.fb.add(offset).cast::<u32>();
            ptr.write_volatile(out_color);
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        for dy in 0..h {
            for dx in 0..w {
                self.pixel(x + dx, y + dy, color);
            }
        }
    }

    fn clear(&mut self, color: u32) {
        self.fill_rect(0, 0, self.width, self.height, color);
    }
}

// ─── Rendu texte ──────────────────────────────────────────────────────────

fn draw_char(gfx: &mut Gfx, x: usize, y: usize, c: u8, color: u32) {
    for &(ch, glyph) in FONT_8X8 {
        if ch == c {
            for (row, &bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits & (0x80 >> col) != 0 {
                        gfx.pixel(x + col, y + row, color);
                    }
                }
            }
            return;
        }
    }
}

fn draw_text(gfx: &mut Gfx, x: usize, y: usize, text: &str, color: u32) {
    let mut cx = x;
    for c in text.bytes() {
        if c == b'\n' {
            break;
        }
        draw_char(gfx, cx, y, c, color);
        cx += CHAR_STEP;
    }
}

fn text_width(text: &str) -> usize {
    text.len() * CHAR_STEP
}

// ─── État des étapes (panneau gauche) ─────────────────────────────────────

const MAX_STEPS: usize = 8;
static mut STEP_TEXTS: [Option<String>; MAX_STEPS] = [None, None, None, None, None, None, None, None];
static mut STEP_STATUS: [Option<bool>; MAX_STEPS] = [None, None, None, None, None, None, None, None];

fn redraw_steps(gfx: &mut Gfx) {
    let left_x = MARGIN;
    let start_y = BANNER_H + 20;
    // SAFETY: environnement UEFI mono-thread, accès via pointeurs bruts (Rust 2024)
    unsafe {
        let texts_ptr = addr_of_mut!(STEP_TEXTS);
        let status_ptr = addr_of_mut!(STEP_STATUS);
        for i in 0..MAX_STEPS {
            if let Some(ref text) = (*texts_ptr)[i] {
                let y = start_y + i * LINE_H * 2 + 4;
                gfx.fill_rect(left_x - 6, y - 3, PANEL_W, LINE_H + 6, C_PANEL);
                draw_text(gfx, left_x, y, text, C_WHITE);
                if let Some(ok) = (*status_ptr)[i] {
                    let status_x = left_x + PANEL_W - 44;
                    if ok {
                        draw_text(gfx, status_x, y, "OK", C_OK);
                    } else {
                        draw_text(gfx, status_x, y, "FAIL", C_FAIL);
                    }
                }
            }
        }
    }
}

fn redraw_right_panel(gfx: &mut Gfx, ctx: &crate::SpooferContext) {
    let left_x = MARGIN;
    let card_y = BANNER_H + 20 + 6 * LINE_H * 2 + 18;
    if card_y + 136 < gfx.height.saturating_sub(BAR_H) {
        gfx.fill_rect(left_x - 6, card_y, PANEL_W, 136, C_PANEL);
        draw_text(gfx, left_x + 4, card_y + 10, "ACTIVE ENGINE STATUS", C_ACCENT);
        draw_text(gfx, left_x + 4, card_y + 30, "* SMBIOS 3.0 (_SM3_) : Patched", C_OK);
        draw_text(gfx, left_x + 4, card_y + 46, "* SMBIOS 2.x (_SM_)  : Patched", C_OK);
        draw_text(gfx, left_x + 4, card_y + 62, "* Entry Checksums    : Verified", C_OK);
        let disk_state = if ctx.disk_spoofed_count > 0 { "Active" } else { "Standby" };
        let disk_line = alloc::format!("* EFI_DISK_INFO Hook : {}", disk_state);
        draw_text(gfx, left_x + 4, card_y + 78, &disk_line, C_CYAN);
        let sync_state = if ctx.patch_report.protocol_synced { "Active" } else { "Direct RAM" };
        let sync_line = alloc::format!("* EFI_SMBIOS Sync    : {}", sync_state);
        draw_text(gfx, left_x + 4, card_y + 94, &sync_line, C_CYAN);
        draw_text(gfx, left_x + 4, card_y + 110, "* Target OS          : bootmgfw.efi", C_GRAY);
    }

    let right_x = MARGIN + PANEL_W + 48;
    let start_y = BANNER_H + 18;
    if right_x >= gfx.width {
        return;
    }

    // Fond panneau droit
    gfx.fill_rect(
        right_x - 12,
        start_y - 10,
        gfx.width.saturating_sub(right_x),
        gfx.height.saturating_sub(BANNER_H + BAR_H + 10),
        C_PANEL,
    );

    // En-tête + version SMBIOS détectée
    if let Some(ref info) = ctx.smbios_info {
        let mode_str = if info.is_64bit { "64-bit _SM3_" } else { "32-bit _SM_" };
        let header = alloc::format!(
            "SMBIOS v{}.{} ({}) -- SPOOF SUMMARY",
            info.major_version,
            info.minor_version,
            mode_str
        );
        draw_text(gfx, right_x, start_y, &header, C_ACCENT);
    } else {
        draw_text(gfx, right_x, start_y, "SMBIOS SPOOF SUMMARY", C_ACCENT);
    }

    let mut uuid_buf = [0u16; 37];
    let mut spoof_uuid_buf = [0u16; 37];
    crate::utils::format_uuid(&ctx.orig.uuid, &mut uuid_buf);
    crate::utils::format_uuid(&ctx.config.uuid, &mut spoof_uuid_buf);

    let line_y = |idx: usize| start_y + 20 + idx * LINE_H;

    draw_field(gfx, right_x, line_y(0), "UUID (orig)", &u16_to_string(&uuid_buf), C_GRAY);
    draw_field(gfx, right_x, line_y(1), "UUID (new) ", &u16_to_string(&spoof_uuid_buf), C_OK);

    let ss = ctx.config.system_serial;
    let bs = ctx.config.baseboard_serial;
    let ps = ctx.config.processor_serial;
    let cs = ctx.config.chassis_serial;
    let ds = ctx.config.disk_serial;

    draw_field_u16(gfx, right_x, line_y(3), "System Serial", &ctx.orig.system_serial, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(4), "  -> new     ", &ss, C_OK);
    draw_field_u16(gfx, right_x, line_y(5), "Baseboard    ", &ctx.orig.baseboard_serial, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(6), "  -> new     ", &bs, C_OK);
    draw_field_u16(gfx, right_x, line_y(7), "Processor    ", &ctx.orig.processor_serial, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(8), "  -> new     ", &ps, C_OK);
    draw_field_u16(gfx, right_x, line_y(9), "Chassis      ", &ctx.orig.chassis_serial, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(10), "  -> new     ", &cs, C_OK);

    // BIOS Info
    let bv = ctx.config.bios_vendor;
    let bver = ctx.config.bios_version;
    let bd = ctx.config.bios_date;
    draw_field_u16(gfx, right_x, line_y(11), "BIOS Vendor  ", &ctx.orig.bios_vendor, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(12), "  -> new     ", &bv, C_OK);
    draw_field_u16(gfx, right_x, line_y(13), "BIOS Version ", &ctx.orig.bios_version, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(14), "  -> new     ", &bver, C_OK);
    draw_field_u16(gfx, right_x, line_y(15), "BIOS Date    ", &ctx.orig.bios_date, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(16), "  -> new     ", &bd, C_OK);

    // Disk Serial (NVMe / SATA / AHCI)
    draw_field_u16(gfx, right_x, line_y(17), "Disk Serial  ", &ctx.orig.first_disk_serial, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(18), "  -> new     ", &ds, C_OK);

    // Type 17 RAM DIMMs + Disks Patched
    let hw_info = alloc::format!(
        "RAM: {} stick(s) | Disks: {} drive(s) (NVMe/SATA)",
        ctx.patch_report.type17_patched,
        ctx.disk_spoofed_count
    );
    draw_field(gfx, right_x, line_y(20), "HW Devices   ", &hw_info, C_CYAN);

    // Persistance status
    let persist_str = alloc::format!(
        "NVRAM: {} | ESP Disk: {} | Source: {}",
        if ctx.persist_status.nvram_ok { "OK" } else { "NO" },
        if ctx.persist_status.disk_ok { "OK" } else { "NO" },
        if ctx.loaded_from_storage { "Saved Profile" } else { "New Profile" }
    );
    draw_field(gfx, right_x, line_y(21), "Persistence  ", &persist_str, C_ACCENT);

    #[cfg(feature = "spoof-mac")]
    {
        let mut orig_mac_buf = [0u16; 18];
        let mut mac_buf = [0u16; 18];
        crate::utils::format_mac(&ctx.orig.mac_address, &mut orig_mac_buf);
        crate::utils::format_mac(&ctx.config.mac_address, &mut mac_buf);
        let mac_str = if ctx.orig.mac_address != [0u8; 6] {
            alloc::format!(
                "{} -> {} (SNP:{})",
                u16_to_string(&orig_mac_buf),
                u16_to_string(&mac_buf),
                ctx.mac_spoofed_count
            )
        } else {
            alloc::format!(
                "{} (SNP:{}, NVRAM:{})",
                u16_to_string(&mac_buf),
                ctx.mac_spoofed_count,
                ctx.nvram_mac_spoofed_count
            )
        };
        draw_field(gfx, right_x, line_y(22), "MAC Address  ", &mac_str, C_ACCENT);
    }

    draw_text(
        gfx,
        right_x,
        line_y(24),
        "All SMBIOS + RAM + Disk + MAC tables patched.",
        C_OK,
    );
}

fn draw_field(gfx: &mut Gfx, x: usize, y: usize, label: &str, value: &str, color: u32) {
    let label_text = alloc::format!("{}: ", label);
    draw_text(gfx, x, y, &label_text, C_GRAY);
    draw_text(gfx, x + text_width(&label_text), y, value, color);
}

fn draw_field_u16(gfx: &mut Gfx, x: usize, y: usize, label: &str, field: &[u16], color: u32) {
    let value = u16_to_string(field);
    let display = if value.is_empty() { "-" } else { value.as_str() };
    draw_field(gfx, x, y, label, display, color);
}

fn u16_to_string(src: &[u16]) -> String {
    let mut s = String::new();
    for &ch in src {
        if ch == 0 {
            break;
        }
        if let Some(c) = char::from_u32(ch as u32) {
            s.push(c);
        }
    }
    s
}

// ─── Barre de progression ─────────────────────────────────────────────────

fn draw_bottom_bar(gfx: &mut Gfx, secs: u64, tick: usize, total: usize, paused: bool) {
    let bar_y = gfx.height.saturating_sub(BAR_H) + 10;

    gfx.fill_rect(0, gfx.height.saturating_sub(BAR_H), gfx.width, BAR_H, C_DARK);

    if paused {
        draw_text(gfx, MARGIN, bar_y + 5, "PAUSED -- Press [P] or [ENTER]", C_CYAN);
    } else {
        let countdown = alloc::format!("Booting Windows in {:2}s", secs);
        draw_text(gfx, MARGIN, bar_y + 5, &countdown, C_ACCENT);

        let bar_w = gfx.width * 42 / 100;
        let bar_x = MARGIN + text_width("Booting Windows in 15s") + 16;
        let bar_fill_y = bar_y + 7;
        let bar_h = 8;

        gfx.fill_rect(bar_x, bar_fill_y, bar_w, bar_h, C_BG);
        let filled = (tick * bar_w).checked_div(total).unwrap_or(bar_w);
        gfx.fill_rect(bar_x, bar_fill_y, filled, bar_h, C_PROGRESS);
    }

    let keys_label = "[ENTER/S] Boot  [R] New IDs  [P] Pause";
    let keys_x = gfx
        .width
        .saturating_sub(MARGIN + text_width(keys_label));
    draw_text(gfx, keys_x, bar_y + 5, keys_label, C_WHITE);
}

// ─── API publique ─────────────────────────────────────────────────────────

pub fn print_banner() {
    if cfg!(feature = "headless") {
        return;
    }
    init_gfx();
    with_gfx(|gfx| {
        gfx.clear(C_BG);
        gfx.fill_rect(0, 0, gfx.width, BANNER_H, C_BANNER_BG);
        let title = "EFI SMBIOS SPOOFER v0.2.0 (SMBIOS 2.x & 3.0 64-bit)";
        let title_x = gfx.width.saturating_sub(text_width(title)) / 2;
        draw_text(gfx, title_x, 16, title, C_ACCENT);
        let sub = "ACROZI x 7ShIkI3 -- Rust Bare-Metal Edition";
        let sub_x = gfx.width.saturating_sub(text_width(sub)) / 2;
        draw_text(gfx, sub_x, 36, sub, C_WHITE);
        let hint = "Type 0/1/2/3/4/11/17 (RAM) + NVMe/SATA Disk Spoof + NVRAM/ESP + Auto-Chainload";
        let hint_x = gfx.width.saturating_sub(text_width(hint)) / 2;
        draw_text(gfx, hint_x, 54, hint, C_GRAY);
    });
}

pub fn print_step(num: u8, total: u8, text: &str) {
    if cfg!(feature = "headless") {
        return;
    }
    let idx = (num as usize).saturating_sub(1).min(MAX_STEPS - 1);
    let step_text = alloc::format!("[{}/{}] {}...", num, total, text);
    unsafe {
        let texts_ptr = addr_of_mut!(STEP_TEXTS);
        let status_ptr = addr_of_mut!(STEP_STATUS);
        (*texts_ptr)[idx] = Some(step_text);
        (*status_ptr)[idx] = None;
    }
    with_gfx(redraw_steps);
}

pub fn print_step_ok() {
    if cfg!(feature = "headless") {
        return;
    }
    unsafe {
        let texts_ptr = addr_of_mut!(STEP_TEXTS);
        let status_ptr = addr_of_mut!(STEP_STATUS);
        for i in (0..MAX_STEPS).rev() {
            if (*texts_ptr)[i].is_some() && (*status_ptr)[i].is_none() {
                (*status_ptr)[i] = Some(true);
                break;
            }
        }
    }
    with_gfx(redraw_steps);
}

pub fn print_step_fail(reason: &str) {
    if cfg!(feature = "headless") {
        return;
    }
    unsafe {
        let texts_ptr = addr_of_mut!(STEP_TEXTS);
        let status_ptr = addr_of_mut!(STEP_STATUS);
        for i in (0..MAX_STEPS).rev() {
            if (*texts_ptr)[i].is_some() && (*status_ptr)[i].is_none() {
                (*status_ptr)[i] = Some(false);
                break;
            }
        }
    }
    with_gfx(|gfx| {
        redraw_steps(gfx);
        let y = BANNER_H + 20 + MAX_STEPS * LINE_H * 2 + 20;
        draw_text(gfx, MARGIN, y, reason, C_FAIL);
    });
}

pub fn print_step_skipped() {
    if cfg!(feature = "headless") {
        return;
    }
    unsafe {
        let texts_ptr = addr_of_mut!(STEP_TEXTS);
        let status_ptr = addr_of_mut!(STEP_STATUS);
        for i in (0..MAX_STEPS).rev() {
            if (*texts_ptr)[i].is_some() && (*status_ptr)[i].is_none() {
                (*status_ptr)[i] = Some(true);
                break;
            }
        }
    }
    with_gfx(redraw_steps);
}

pub fn print_summary(ctx: &crate::SpooferContext) {
    if cfg!(feature = "headless") {
        return;
    }
    with_gfx(|gfx| redraw_right_panel(gfx, ctx));
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    BootNow,
    Regenerate,
    TogglePause,
}

/// Compte à rebours interactif (5 secondes par défaut pour un boot rapide).
/// Retourne `Some(true)` si l'utilisateur appuie sur `[R]` (régénérer les IDs),
/// `Some(false)` s'il appuie sur `[ENTER]`/`[S]` (boot immédiat), ou `None` au timeout.
/// Appuyer sur `[P]` met en pause / reprend le compte à rebours.
pub fn interactive_menu() -> Option<bool> {
    if cfg!(feature = "headless") {
        return Some(false);
    }

    let auto_boot_secs: u64 = 5;
    let poll_ms: u64 = 100;
    let total_ticks = (auto_boot_secs * 1000) / poll_ms;
    let mut tick: u64 = 0;
    let mut paused = false;

    while tick <= total_ticks {
        let remaining = total_ticks - tick;
        let secs = (remaining * poll_ms).div_ceil(1000);

        with_gfx(|gfx| draw_bottom_bar(gfx, secs, tick as usize, total_ticks as usize, paused));

        uefi::boot::stall(core::time::Duration::from_millis(poll_ms));

        if let Some(action) = check_key() {
            match action {
                KeyAction::Regenerate => return Some(true),
                KeyAction::BootNow => return Some(false),
                KeyAction::TogglePause => {
                    paused = !paused;
                }
            }
        }

        if !paused {
            tick += 1;
        }
    }
    None
}

fn check_key() -> Option<KeyAction> {
    uefi::system::with_stdin(|con_in| {
        if let Ok(Some(key)) = con_in.read_key() {
            if let uefi::proto::console::text::Key::Printable(c) = key {
                let ch: u16 = c.into();
                if ch == (b'r' as u16) || ch == (b'R' as u16) {
                    return Some(KeyAction::Regenerate);
                }
                if ch == (b'p' as u16) || ch == (b'P' as u16) {
                    return Some(KeyAction::TogglePause);
                }
            }
            Some(KeyAction::BootNow) // Toute autre touche (Enter, Space, S...) lance le boot immédiat
        } else {
            None
        }
    })
}

pub fn fatal_error(msg: &str) {
    if cfg!(feature = "headless") {
        return;
    }
    with_gfx(|gfx| {
        let y = gfx.height / 2 - 40;
        gfx.fill_rect(gfx.width / 4, y - 5, gfx.width / 2, 60, C_FAIL);
        let x = gfx.width.saturating_sub(text_width(msg)) / 2;
        draw_text(gfx, x, y + 15, msg, C_WHITE);
        let hint = "Press any key to continue boot...";
        let hx = gfx.width.saturating_sub(text_width(hint)) / 2;
        draw_text(gfx, hx, y + 45, hint, C_GRAY);
    });
}

pub fn wait_any_key() {
    if cfg!(feature = "headless") {
        return;
    }
    uefi::system::with_stdin(|con_in| {
        loop {
            if let Ok(Some(_)) = con_in.read_key() {
                break;
            }
            uefi::boot::stall(core::time::Duration::from_millis(100));
        }
    });
}

pub fn colored(_text: &str, _fg: Color) {}
