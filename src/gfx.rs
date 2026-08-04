//! Module de rendu framebuffer GOP — remplace la console texte.
//!
//! Résout le bug hardware AMI Aptio : `SetAttribute()` est ignoré sur les
//! firmwares ASUS TUF/ROG. On dessine directement dans le framebuffer.
//!
//! En mode headless (`--features headless`), toutes les fonctions sont no-op.
//! Les signatures publiques sont identiques à `ui.rs` → `main.rs` ne change pas.

use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};
use uefi::proto::console::text::Color;
use core::sync::atomic::{AtomicBool, Ordering};
extern crate alloc;
use alloc::string::String;

// ─── État global du framebuffer ─────────────────────────────────────────

static mut GFX: Option<Gfx> = None;
static GFX_READY: AtomicBool = AtomicBool::new(false);

struct Gfx {
    fb: *mut u8,          // framebuffer linéaire
    width: usize,
    height: usize,
    stride: usize,        // pixels par ligne (peut être > width)
    #[allow(dead_code)]
    pixel_format: PixelFormat,
}

// ─── Couleurs (BGRx — format framebuffer standard UEFI) ──────────────────

const C_BG:       u32 = 0x002E1A1A; // fond sombre (bleu nuit)
const C_ACCENT:   u32 = 0x00FFBF00; // orange/ambre
const C_OK:       u32 = 0x0000CC00; // vert
const C_FAIL:     u32 = 0x000000CC; // rouge
const C_WHITE:    u32 = 0x00FFFFFF;
const C_GRAY:     u32 = 0x00888888;
const C_DARK:     u32 = 0x00333333;
const C_PANEL:    u32 = 0x00332222; // fond panneau
const C_PROGRESS: u32 = 0x0044CC00; // vert progression
const C_BANNER_BG: u32 = 0x00332211; // fond bannière

// ─── Layout ──────────────────────────────────────────────────────────────

const FONT_W: usize = 8;
const FONT_H: usize = 8;
const CHAR_SPACING: usize = 1; // pixels entre caractères
const CHAR_STEP: usize = FONT_W + CHAR_SPACING; // 9
const LINE_H: usize = FONT_H + 4; // espacement vertical entre lignes
const MARGIN: usize = 40;
const BANNER_H: usize = 75;
const BAR_H: usize = 55;
const PANEL_W: usize = 380;

// ─── Police bitmap 8×8 — ASCII 32-126 ────────────────────────────────────

const FONT_8X8: &[(u8, [u8; 8])] = &[
    (b' ', [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00]),
    (b'!', [0x18,0x3C,0x3C,0x18,0x18,0x00,0x18,0x00]),
    (b'"', [0x6C,0x6C,0x00,0x00,0x00,0x00,0x00,0x00]),
    (b'#', [0x6C,0x6C,0xFE,0x6C,0xFE,0x6C,0x6C,0x00]),
    (b'$', [0x18,0x3E,0x60,0x3C,0x06,0x7C,0x18,0x00]),
    (b'%', [0x00,0x63,0x66,0x0C,0x18,0x33,0x63,0x00]),
    (b'&', [0x38,0x6C,0x38,0x76,0xDC,0xCC,0x76,0x00]),
    (b'\'',[0x18,0x18,0x30,0x00,0x00,0x00,0x00,0x00]),
    (b'(', [0x0C,0x18,0x30,0x30,0x30,0x18,0x0C,0x00]),
    (b')', [0x30,0x18,0x0C,0x0C,0x0C,0x18,0x30,0x00]),
    (b'*', [0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00]),
    (b'+', [0x00,0x18,0x18,0x7E,0x18,0x18,0x00,0x00]),
    (b',', [0x00,0x00,0x00,0x00,0x18,0x18,0x30,0x00]),
    (b'-', [0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00]),
    (b'.', [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00]),
    (b'/', [0x06,0x0C,0x18,0x30,0x60,0x40,0x00,0x00]),
    (b'0', [0x3C,0x66,0x6E,0x76,0x66,0x66,0x3C,0x00]),
    (b'1', [0x18,0x38,0x18,0x18,0x18,0x18,0x7E,0x00]),
    (b'2', [0x3C,0x66,0x06,0x0C,0x30,0x60,0x7E,0x00]),
    (b'3', [0x3C,0x66,0x06,0x1C,0x06,0x66,0x3C,0x00]),
    (b'4', [0x06,0x0E,0x1E,0x66,0x7F,0x06,0x06,0x00]),
    (b'5', [0x7E,0x60,0x7C,0x06,0x06,0x66,0x3C,0x00]),
    (b'6', [0x3C,0x66,0x60,0x7C,0x66,0x66,0x3C,0x00]),
    (b'7', [0x7E,0x66,0x0C,0x18,0x18,0x18,0x18,0x00]),
    (b'8', [0x3C,0x66,0x66,0x3C,0x66,0x66,0x3C,0x00]),
    (b'9', [0x3C,0x66,0x66,0x3E,0x06,0x66,0x3C,0x00]),
    (b':', [0x00,0x18,0x18,0x00,0x18,0x18,0x00,0x00]),
    (b';', [0x00,0x18,0x18,0x00,0x18,0x18,0x30,0x00]),
    (b'<', [0x06,0x0C,0x18,0x30,0x18,0x0C,0x06,0x00]),
    (b'=', [0x00,0x00,0x7E,0x00,0x7E,0x00,0x00,0x00]),
    (b'>', [0x60,0x30,0x18,0x0C,0x18,0x30,0x60,0x00]),
    (b'?', [0x3C,0x66,0x0C,0x18,0x18,0x00,0x18,0x00]),
    (b'@', [0x3C,0x66,0x6E,0x6A,0x6E,0x60,0x3C,0x00]),
    (b'A', [0x18,0x3C,0x66,0x7E,0x66,0x66,0x66,0x00]),
    (b'B', [0x7C,0x66,0x66,0x7C,0x66,0x66,0x7C,0x00]),
    (b'C', [0x3C,0x66,0x60,0x60,0x60,0x66,0x3C,0x00]),
    (b'D', [0x78,0x6C,0x66,0x66,0x66,0x6C,0x78,0x00]),
    (b'E', [0x7E,0x60,0x60,0x78,0x60,0x60,0x7E,0x00]),
    (b'F', [0x7E,0x60,0x60,0x78,0x60,0x60,0x60,0x00]),
    (b'G', [0x3C,0x66,0x60,0x6E,0x66,0x66,0x3C,0x00]),
    (b'H', [0x66,0x66,0x66,0x7E,0x66,0x66,0x66,0x00]),
    (b'I', [0x3C,0x18,0x18,0x18,0x18,0x18,0x3C,0x00]),
    (b'J', [0x1E,0x0C,0x0C,0x0C,0x0C,0x6C,0x38,0x00]),
    (b'K', [0x66,0x6C,0x78,0x70,0x78,0x6C,0x66,0x00]),
    (b'L', [0x60,0x60,0x60,0x60,0x60,0x60,0x7E,0x00]),
    (b'M', [0x63,0x77,0x7F,0x6B,0x63,0x63,0x63,0x00]),
    (b'N', [0x66,0x76,0x7E,0x7E,0x6E,0x66,0x66,0x00]),
    (b'O', [0x3C,0x66,0x66,0x66,0x66,0x66,0x3C,0x00]),
    (b'P', [0x7C,0x66,0x66,0x7C,0x60,0x60,0x60,0x00]),
    (b'Q', [0x3C,0x66,0x66,0x66,0x66,0x3C,0x0E,0x00]),
    (b'R', [0x7C,0x66,0x66,0x7C,0x78,0x6C,0x66,0x00]),
    (b'S', [0x3C,0x66,0x60,0x3C,0x06,0x66,0x3C,0x00]),
    (b'T', [0x7E,0x18,0x18,0x18,0x18,0x18,0x18,0x00]),
    (b'U', [0x66,0x66,0x66,0x66,0x66,0x66,0x3C,0x00]),
    (b'V', [0x66,0x66,0x66,0x66,0x66,0x3C,0x18,0x00]),
    (b'W', [0x63,0x63,0x63,0x6B,0x7F,0x77,0x63,0x00]),
    (b'X', [0x66,0x66,0x3C,0x18,0x3C,0x66,0x66,0x00]),
    (b'Y', [0x66,0x66,0x66,0x3C,0x18,0x18,0x18,0x00]),
    (b'Z', [0x7E,0x06,0x0C,0x18,0x30,0x60,0x7E,0x00]),
    (b'[', [0x3C,0x30,0x30,0x30,0x30,0x30,0x3C,0x00]),
    (b'\\',[0x60,0x30,0x18,0x0C,0x06,0x02,0x00,0x00]),
    (b']', [0x3C,0x0C,0x0C,0x0C,0x0C,0x0C,0x3C,0x00]),
    (b'^', [0x18,0x3C,0x66,0x00,0x00,0x00,0x00,0x00]),
    (b'_', [0x00,0x00,0x00,0x00,0x00,0x00,0x7E,0x00]),
    (b'`', [0x30,0x18,0x0C,0x00,0x00,0x00,0x00,0x00]),
    (b'a', [0x00,0x00,0x3C,0x06,0x3E,0x66,0x3E,0x00]),
    (b'b', [0x60,0x60,0x7C,0x66,0x66,0x66,0x7C,0x00]),
    (b'c', [0x00,0x00,0x3C,0x66,0x60,0x66,0x3C,0x00]),
    (b'd', [0x06,0x06,0x3E,0x66,0x66,0x66,0x3E,0x00]),
    (b'e', [0x00,0x00,0x3C,0x66,0x7E,0x60,0x3C,0x00]),
    (b'f', [0x0E,0x18,0x3E,0x18,0x18,0x18,0x18,0x00]),
    (b'g', [0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x3C]),
    (b'h', [0x60,0x60,0x7C,0x66,0x66,0x66,0x66,0x00]),
    (b'i', [0x18,0x00,0x38,0x18,0x18,0x18,0x3C,0x00]),
    (b'j', [0x06,0x00,0x06,0x06,0x06,0x66,0x3C,0x00]),
    (b'k', [0x60,0x60,0x66,0x6C,0x78,0x6C,0x66,0x00]),
    (b'l', [0x38,0x18,0x18,0x18,0x18,0x18,0x3C,0x00]),
    (b'm', [0x00,0x00,0x63,0x77,0x7F,0x6B,0x63,0x00]),
    (b'n', [0x00,0x00,0x7C,0x66,0x66,0x66,0x66,0x00]),
    (b'o', [0x00,0x00,0x3C,0x66,0x66,0x66,0x3C,0x00]),
    (b'p', [0x00,0x00,0x7C,0x66,0x66,0x7C,0x60,0x60]),
    (b'q', [0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x06]),
    (b'r', [0x00,0x00,0x7C,0x66,0x60,0x60,0x60,0x00]),
    (b's', [0x00,0x00,0x3E,0x60,0x3C,0x06,0x7C,0x00]),
    (b't', [0x18,0x18,0x7E,0x18,0x18,0x18,0x0E,0x00]),
    (b'u', [0x00,0x00,0x66,0x66,0x66,0x66,0x3E,0x00]),
    (b'v', [0x00,0x00,0x66,0x66,0x66,0x3C,0x18,0x00]),
    (b'w', [0x00,0x00,0x63,0x63,0x6B,0x7F,0x36,0x00]),
    (b'x', [0x00,0x00,0x66,0x3C,0x18,0x3C,0x66,0x00]),
    (b'y', [0x00,0x00,0x66,0x66,0x66,0x3E,0x06,0x3C]),
    (b'z', [0x00,0x00,0x7E,0x0C,0x18,0x30,0x7E,0x00]),
    (b'{', [0x0E,0x18,0x18,0x70,0x18,0x18,0x0E,0x00]),
    (b'|', [0x18,0x18,0x18,0x18,0x18,0x18,0x18,0x00]),
    (b'}', [0x70,0x18,0x18,0x0E,0x18,0x18,0x70,0x00]),
    (b'~', [0x76,0xDC,0x00,0x00,0x00,0x00,0x00,0x00]),
];

// ─── Accès au Gfx ────────────────────────────────────────────────────────

/// Initialise le GOP et stocke le framebuffer global.
/// Appelé une seule fois au début de `print_banner()`.
fn init_gfx() {
    if GFX_READY.load(Ordering::Relaxed) { return; }

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
    // jusqu'à l'exit des boot services. On est single-thread.
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
    // SAFETY: single-thread UEFI, pas de concurrence.
    // On utilise un pointeur brut pour éviter static_mut_refs (Rust 2024).
    let ptr: *mut Option<Gfx> = core::ptr::addr_of_mut!(GFX);
    unsafe { (*ptr).as_mut() }
}

fn with_gfx<F: FnOnce(&mut Gfx)>(f: F) {
    if cfg!(feature = "headless") { return; }
    if let Some(gfx) = get_gfx() {
        f(gfx);
    }
}

// ─── Impl Gfx ────────────────────────────────────────────────────────────

impl Gfx {
    /// Écrit un pixel. Le format dépend du pixel_format détecté.
    fn pixel(&mut self, x: usize, y: usize, color: u32) {
        if x >= self.width || y >= self.height { return; }
        let offset = (y * self.stride + x) * 4;
        // SAFETY: offset vérifié dans les bornes, pointeur valide
        unsafe {
            let ptr = self.fb.add(offset) as *mut u32;
            ptr.write_volatile(color);
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
            for row in 0..8 {
                for col in 0..8 {
                    if glyph[row] & (0x80 >> col) != 0 {
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
        if c == b'\n' { break; }
        draw_char(gfx, cx, y, c, color);
        cx += CHAR_STEP;
    }
}

fn text_width(text: &str) -> usize {
    text.len() * CHAR_STEP
}

// ─── État des étapes (pour redessiner après OK/FAIL) ──────────────────────

static mut STEP_TEXTS: [Option<alloc::string::String>; 4] = [None, None, None, None];
static mut STEP_STATUS: [Option<bool>; 4] = [None, None, None, None]; // Some(true)=OK, Some(false)=FAIL

fn redraw_steps(gfx: &mut Gfx) {
    let left_x = MARGIN;
    let start_y = BANNER_H + 20;
    // SAFETY: single-thread
    unsafe {
        for i in 0..4 {
            if let Some(ref text) = STEP_TEXTS[i] {
                let y = start_y + i * LINE_H * 2 + 4;
                // Fond
                gfx.fill_rect(left_x - 5, y - 2, PANEL_W, LINE_H + 4, C_PANEL);
                // Texte de l'étape
                draw_text(gfx, left_x, y, text, C_WHITE);
                // Statut
                if let Some(ok) = STEP_STATUS[i] {
                    let status_x = left_x + PANEL_W - 40;
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
    let right_x = MARGIN + PANEL_W + 60;
    let start_y = BANNER_H + 20;

    // Fond panneau droit
    gfx.fill_rect(right_x - 10, start_y - 15, gfx.width - right_x, gfx.height - BANNER_H - BAR_H - 10, C_PANEL);

    // Titre
    draw_text(gfx, right_x, start_y, "SMBIOS SPOOF SUMMARY", C_ACCENT);

    let mut uuid_buf = [0u16; 37];
    let mut spoof_uuid_buf = [0u16; 37];
    crate::utils::format_uuid(&ctx.orig_uuid, &mut uuid_buf);
    crate::utils::format_uuid(&ctx.config.uuid, &mut spoof_uuid_buf);

    let line_y = |idx: usize| start_y + 30 + idx * LINE_H;

    draw_field(gfx, right_x, line_y(0), "UUID", &u16_to_string(&uuid_buf), C_GRAY);
    draw_field(gfx, right_x, line_y(1), "UUID (new)", &u16_to_string(&spoof_uuid_buf), C_OK);

    let ss = ctx.config.system_serial;
    let bs = ctx.config.baseboard_serial;
    let ps = ctx.config.processor_serial;
    let cs = ctx.config.chassis_serial;

    draw_field_u16(gfx, right_x, line_y(3), "System Serial", &ctx.orig_system_serial, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(4), "  -> new", &ss, C_OK);
    draw_field_u16(gfx, right_x, line_y(6), "Baseboard", &ctx.orig_baseboard_serial, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(7), "  -> new", &bs, C_OK);
    draw_field_u16(gfx, right_x, line_y(9), "Processor", &ctx.orig_processor_serial, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(10), "  -> new", &ps, C_OK);
    draw_field_u16(gfx, right_x, line_y(12), "Chassis", &ctx.orig_chassis_serial, C_GRAY);
    draw_field_u16(gfx, right_x, line_y(13), "  -> new", &cs, C_OK);

    draw_text(gfx, right_x, line_y(15), "All spoofs applied.", C_OK);
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
        if ch == 0 { break; }
        if let Some(c) = char::from_u32(ch as u32) {
            s.push(c);
        }
    }
    s
}

// ─── Barre de progression ─────────────────────────────────────────────────

fn draw_bottom_bar(gfx: &mut Gfx, secs: u64, tick: usize, total: usize) {
    let bar_y = gfx.height - BAR_H + 10;

    // Fond de la barre
    gfx.fill_rect(0, gfx.height - BAR_H, gfx.width, BAR_H, C_DARK);

    // Texte compte à rebours
    let countdown = alloc::format!("Auto-boot in {}s", secs);
    draw_text(gfx, MARGIN, bar_y + 5, &countdown, C_ACCENT);

    // Barre de progression
    let bar_w = gfx.width * 60 / 100;
    let bar_x = MARGIN + text_width("Auto-boot in 5s") + 20;
    let bar_fill_y = bar_y + 14;
    let bar_h = 6;

    // Fond barre
    gfx.fill_rect(bar_x, bar_fill_y, bar_w, bar_h, C_DARK);
    // Partie remplie
    let filled = if total > 0 { (tick * bar_w) / total } else { bar_w };
    gfx.fill_rect(bar_x, bar_fill_y, filled, bar_h, C_PROGRESS);

    // Touches disponibles
    let keys_x = gfx.width - MARGIN - text_width("[ENTER] Boot [R] Reset");
    draw_text(gfx, keys_x, bar_y + 5, "[ENTER] Boot  [R] Reset", C_GRAY);
}

// ─── API publique (identique à ui.rs) ─────────────────────────────────────

pub fn print_banner() {
    if cfg!(feature = "headless") { return; }
    init_gfx();
    with_gfx(|gfx| {
        gfx.clear(C_BG);
        // Bannière
        gfx.fill_rect(0, 0, gfx.width, BANNER_H, C_BANNER_BG);
        let title = "EFI SMBIOS SPOOFER v4";
        let title_x = (gfx.width - text_width(title)) / 2;
        draw_text(gfx, title_x, 18, title, C_ACCENT);
        let sub = "ACROZI x 7ShIkI3 — Rust Port";
        let sub_x = (gfx.width - text_width(sub)) / 2;
        draw_text(gfx, sub_x, 40, sub, C_GRAY);
        let hint = "GOP Framebuffer Mode";
        let hint_x = (gfx.width - text_width(hint)) / 2;
        draw_text(gfx, hint_x, 56, hint, C_DARK);
    });
}

pub fn print_step(num: u8, text: &str) {
    if cfg!(feature = "headless") { return; }
    let step_text = alloc::format!("[{}/4] {}...", num, text);
    // SAFETY: single-thread
    unsafe {
        STEP_TEXTS[num as usize - 1] = Some(step_text);
        STEP_STATUS[num as usize - 1] = None;
    }
    with_gfx(|gfx| redraw_steps(gfx));
}

pub fn print_step_ok() {
    if cfg!(feature = "headless") { return; }
    // Marque la dernière étape comme OK
    unsafe {
        for i in (0..4).rev() {
            if STEP_TEXTS[i].is_some() && STEP_STATUS[i].is_none() {
                STEP_STATUS[i] = Some(true);
                break;
            }
        }
    }
    with_gfx(|gfx| redraw_steps(gfx));
}

pub fn print_step_fail(reason: &str) {
    if cfg!(feature = "headless") { return; }
    unsafe {
        for i in (0..4).rev() {
            if STEP_TEXTS[i].is_some() && STEP_STATUS[i].is_none() {
                STEP_STATUS[i] = Some(false);
                break;
            }
        }
    }
    with_gfx(|gfx| {
        redraw_steps(gfx);
        let y = BANNER_H + 20 + 4 * LINE_H * 2 + 20;
        draw_text(gfx, MARGIN, y, reason, C_FAIL);
    });
}

pub fn print_step_skipped() {
    if cfg!(feature = "headless") { return; }
    // Marque comme OK mais affiche "SKIP"
    unsafe {
        for i in (0..4).rev() {
            if STEP_TEXTS[i].is_some() && STEP_STATUS[i].is_none() {
                STEP_STATUS[i] = Some(true);
                break;
            }
        }
    }
    with_gfx(|gfx| redraw_steps(gfx));
}

pub fn print_summary(ctx: &crate::SpooferContext) {
    if cfg!(feature = "headless") { return; }
    with_gfx(|gfx| redraw_right_panel(gfx, ctx));
}

pub fn interactive_menu() -> Option<bool> {
    if cfg!(feature = "headless") {
        return Some(false);
    }

    let auto_boot_secs: u64 = 15;
    let poll_ms: u64 = 125;
    let total_ticks = (auto_boot_secs * 1000) / poll_ms;

    for tick in 0..=total_ticks {
        let remaining = total_ticks - tick;
        let secs = (remaining * poll_ms + 999) / 1000;

        // Redessiner la barre de progression
        with_gfx(|gfx| draw_bottom_bar(gfx, secs, tick as usize, total_ticks as usize));

        uefi::boot::stall(core::time::Duration::from_millis(poll_ms));

        // Vérifier les touches
        if let Some(action) = check_key() {
            return Some(action);
        }
    }
    None // timeout
}

fn check_key() -> Option<bool> {
    uefi::system::with_stdin(|con_in| {
        if let Ok(Some(key)) = con_in.read_key() {
            match key {
                uefi::proto::console::text::Key::Printable(c) => {
                    let ch: u16 = c.into();
                    if ch == (b'r' as u16) || ch == (b'R' as u16) {
                        return Some(true);
                    }
                }
                _ => {}
            }
            Some(false) // toute autre touche → boot
        } else {
            None
        }
    })
}

pub fn fatal_error(msg: &str) {
    if cfg!(feature = "headless") { return; }
    with_gfx(|gfx| {
        let y = gfx.height / 2 - 40;
        gfx.fill_rect(gfx.width / 4, y - 5, gfx.width / 2, 60, C_FAIL);
        let x = (gfx.width - text_width(msg)) / 2;
        draw_text(gfx, x, y + 15, msg, C_WHITE);
        let hint = "Press any key to continue boot...";
        let hx = (gfx.width - text_width(hint)) / 2;
        draw_text(gfx, hx, y + 45, hint, C_GRAY);
    });
}

pub fn wait_any_key() {
    if cfg!(feature = "headless") { return; }
    uefi::system::with_stdin(|con_in| {
        loop {
            if let Ok(Some(_)) = con_in.read_key() { break; }
            uefi::boot::stall(core::time::Duration::from_millis(100));
        }
    });
}

pub fn colored(_text: &str, _fg: Color) {
    // GOP ignore les couleurs console — on contrôle les pixels directement
}
