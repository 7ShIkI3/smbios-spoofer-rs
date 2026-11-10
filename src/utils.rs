//! Utilitaires : génération aléatoire, format UUID, détection constructeur et numéros de série.
//!
//! ## Ce que ce module fournit :
//! - Générateur pseudo-aléatoire (LCG + entropie RTC UEFI + compteur TSC CPU `rdtsc`)
//! - Génération et formatage d'UUID v4 (RFC 4122 + encodage mixed-endian SMBIOS 2.6+)
//! - Détection des valeurs placeholders (`"Default string"`, `"To be filled by O.E.M."`, etc.)
//! - Détection des numéros de série hexadécimaux et par constructeur (Dell, Lenovo, HP, Apple, ASUS, MSI, Gigabyte, Acer)
//! - Génération de chaînes BIOS, Asset Tag, SKU et RAM format-matched (conservation de la longueur exacte en mémoire)

#[cfg(target_os = "uefi")]
use core::time::Duration;

// ─── Générateur aléatoire (LCG + TSC) ───────────────────────────────────────

/// Graine du générateur LCG.
/// SAFETY: Exécution single-thread en environnement UEFI DXE/BootServices.
static mut RANDOM_SEED: u32 = 0xDEAD_BEEF;

/// Lit le compteur de cycles CPU (Time Stamp Counter) sur x86_64 pour l'entropie.
#[inline]
fn read_tsc_u32() -> u32 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let tsc = core::arch::x86_64::_rdtsc();
        (tsc as u32) ^ ((tsc >> 32) as u32)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0x1234_5678
    }
}

/// Initialise la graine sur l'hôte (tests unitaires).
#[cfg(not(target_os = "uefi"))]
pub fn random_init() {
    let tsc = read_tsc_u32();
    unsafe {
        RANDOM_SEED = if tsc != 0 { tsc } else { 0xDEAD_BEEF };
    }
}

/// Initialise la graine avec l'horloge système UEFI et le TSC matériel.
#[cfg(target_os = "uefi")]
pub fn random_init() {
    let mut seed: u32 = read_tsc_u32();

    if let Ok(time) = uefi::runtime::get_time() {
        let date_part = (time.year() as u32)
            .wrapping_mul(10000)
            .wrapping_add(time.month() as u32 * 100)
            .wrapping_add(time.day() as u32);
        let time_part = (time.hour() as u32)
            .wrapping_mul(3600)
            .wrapping_add(time.minute() as u32 * 60)
            .wrapping_add(time.second() as u32);
        seed ^= date_part.rotate_left(7) ^ time_part.rotate_right(3) ^ time.nanosecond();
    }

    if seed == 0 {
        seed = 0xDEAD_BEEF;
    }

    unsafe {
        RANDOM_SEED = seed;
    }
}

/// Réinjecte de l'entropie matérielle (appelé lors d'un Reset `[R]` interactif).
pub fn reseed_entropy() {
    let tsc = read_tsc_u32();
    unsafe {
        RANDOM_SEED = RANDOM_SEED
            .wrapping_mul(1664525)
            .wrapping_add(1013904223)
            ^ tsc;
        if RANDOM_SEED == 0 {
            RANDOM_SEED = 0xCAFE_BABE;
        }
    }
}

/// Définit explicitement la graine (utilisé par les tests unitaires déterministes).
#[allow(dead_code)]
pub fn random_set_seed(seed: u32) {
    unsafe {
        RANDOM_SEED = seed;
    }
}

/// Générateur LCG (constantes glibc). Retourne un `u32` pseudo-aléatoire.
pub fn random_u32() -> u32 {
    unsafe {
        RANDOM_SEED = RANDOM_SEED.wrapping_mul(1103515245).wrapping_add(12345);
        RANDOM_SEED
    }
}

/// Sélectionne un index aléatoire dans `0..len` en utilisant les bits de poids fort du LCG.
#[inline]
fn pick_index(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (((random_u32() >> 8) as usize) ^ (random_u32() as usize)) % len
}

/// Remplit un buffer avec des octets aléatoires.
pub fn random_bytes(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        *byte = ((random_u32() >> 16) & 0xFF) as u8;
    }
}

// ─── UUID v4 & Mixed-Endian SMBIOS ─────────────────────────────────────────

/// Génère un UUID v4 (RFC 4122) aléatoire.
/// - byte 6 : bits 4..7 = `0100` (version 4)
/// - byte 8 : bits 6..7 = `10` (variant RFC 4122)
pub fn generate_uuid(uuid: &mut [u8; 16]) {
    random_bytes(uuid);
    uuid[6] = (uuid[6] & 0x0F) | 0x40;
    uuid[8] = (uuid[8] & 0x3F) | 0x80;
}

/// Convertit un UUID RFC 4122 vers la représentation mémoire SMBIOS 2.6+ (mixed-endian).
/// Les 3 premiers champs (4 + 2 + 2 octets) sont en little-endian, les 8 derniers en big-endian.
pub fn uuid_to_smbios_bytes(uuid: &[u8; 16]) -> [u8; 16] {
    [
        uuid[3], uuid[2], uuid[1], uuid[0], // time_low (LE)
        uuid[5], uuid[4],                   // time_mid (LE)
        uuid[7], uuid[6],                   // time_hi_and_version (LE)
        uuid[8], uuid[9],                   // clock_seq (BE)
        uuid[10], uuid[11], uuid[12], uuid[13], uuid[14], uuid[15], // node (BE)
    ]
}

/// Génère un UUID v4 similaire à l'original (conserve les 3 premiers octets constructeur, change le reste).
pub fn generate_uuid_similar(orig: &[u8; 16], out: &mut [u8; 16]) {
    generate_uuid(out);
    if orig.iter().any(|&b| b != 0 && b != 0xFF) {
        out[0] = orig[0];
        out[1] = orig[1];
    }
    out[6] = (out[6] & 0x0F) | 0x40;
    out[8] = (out[8] & 0x3F) | 0x80;
    if out == orig {
        out[15] = out[15].wrapping_add(1);
    }
}

/// Formate un UUID en chaîne UTF-16 (`"XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"`).
pub fn format_uuid(uuid: &[u8; 16], buf: &mut [u16; 37]) {
    const HEX: &[u8] = b"0123456789ABCDEF";

    let mut pos = 0;
    for (i, &byte) in uuid.iter().enumerate() {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            buf[pos] = b'-' as u16;
            pos += 1;
        }
        buf[pos] = HEX[(byte >> 4) as usize] as u16;
        pos += 1;
        buf[pos] = HEX[(byte & 0x0F) as usize] as u16;
        pos += 1;
    }
    buf[36] = 0;
}

// ─── Jeux de caractères ────────────────────────────────────────────────────

/// Caractères alphanumériques lisibles (sans O/0, I/1, L ambigus).
const SERIAL_CHARSET: &[u8] = b"123456789ABCDEFGHJKMNPQRSTUVWXYZ";
const DIGITS: &[u8] = b"0123456789";
const DIGITS_NO_ZERO: &[u8] = b"123456789";
const UPPER_LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
const LOWER_LETTERS: &[u8] = b"abcdefghjkmnpqrstuvwxyz";
const HEX_UPPER: &[u8] = b"0123456789ABCDEF";

// ─── Utilitaires chaînes UTF-16 ────────────────────────────────────────────

/// Longueur d'une chaîne UTF-16 terminée par null (sans les espaces de fin).
pub fn str_len_u16(s: &[u16]) -> usize {
    s.iter().position(|&c| c == 0).unwrap_or(s.len())
}

/// Copie une chaîne UTF-16 avec terminateur null garanti.
pub fn str_copy_u16(dst: &mut [u16], src: &[u16]) {
    if dst.is_empty() {
        return;
    }
    let max = dst.len() - 1;
    let mut i = 0;
    while i < max && i < src.len() && src[i] != 0 {
        dst[i] = src[i];
        i += 1;
    }
    dst[i] = 0;
}

/// Compare deux slices `u16`.
pub fn slice_eq_u16(a: &[u16], b: &[u16]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

#[inline]
fn to_lower_ascii(c: u16) -> u8 {
    let b = (c & 0xFF) as u8;
    if b.is_ascii_uppercase() {
        b + 32
    } else {
        b
    }
}

fn starts_with_ignore_case(s: &[u16], prefix: &[u8]) -> bool {
    let len = str_len_u16(s);
    if len < prefix.len() {
        return false;
    }
    for i in 0..prefix.len() {
        if to_lower_ascii(s[i]) != prefix[i].to_ascii_lowercase() {
            return false;
        }
    }
    true
}

fn equals_ignore_case(s: &[u16], target: &[u8]) -> bool {
    let len = str_len_u16(s);
    if len != target.len() {
        return false;
    }
    starts_with_ignore_case(s, target)
}

fn contains_ignore_case(s: &[u16], needle: &[u8]) -> bool {
    let len = str_len_u16(s);
    if needle.is_empty() || len < needle.len() {
        return false;
    }
    for start in 0..=(len - needle.len()) {
        let mut matched = true;
        for j in 0..needle.len() {
            if to_lower_ascii(s[start + j]) != needle[j].to_ascii_lowercase() {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

/// Vérifie si un numéro de série est une valeur placeholder OEM
/// (`"Default string"`, `"To be filled by O.E.M."`, `"System Serial Number"`, etc.).
pub fn is_placeholder(serial: &[u16]) -> bool {
    let len = str_len_u16(serial);
    if len == 0 {
        return true;
    }

    let substrings: &[&[u8]] = &[
        b"default",
        b"to be filled",
        b"not specified",
        b"system serial",
        b"system product",
        b"baseboard serial",
        b"chassis serial",
        b"asset tag",
        b"asset",
        b"no asset",
        b"oem",
    ];
    for sub in substrings {
        if starts_with_ignore_case(serial, sub) || contains_ignore_case(serial, sub) {
            return true;
        }
    }

    let exacts: &[&[u8]] = &[
        b"unknown",
        b"none",
        b"null",
        b"n/a",
        b"0",
        b"00000000",
        b"000000000000",
        b"123456789",
        b"x",
        b"xxx",
    ];
    for e in exacts {
        if equals_ignore_case(serial, e) {
            return true;
        }
    }

    false
}

/// Détecte si un numéro de série (>= 8 caractères alphanumériques) est purement hexadécimal.
pub fn is_likely_hex_serial(original: &[u16]) -> bool {
    let len = str_len_u16(original);
    if len < 8 {
        return false;
    }
    let mut alnum_count = 0usize;
    let mut hex_count = 0usize;

    for &ch in &original[..len] {
        let b = (ch & 0xFF) as u8;
        if b.is_ascii_alphanumeric() {
            alnum_count += 1;
            if b.is_ascii_hexdigit() {
                hex_count += 1;
            }
        }
    }

    alnum_count >= 8 && alnum_count == hex_count
}

// ─── Génération de numéros de série ────────────────────────────────────────

/// Génère un numéro de série aléatoire de longueur donnée (`1..=63`).
pub fn generate_serial(serial: &mut [u16; 64], length: usize) {
    let len = length.clamp(1, 63);
    for item in serial.iter_mut().take(len) {
        let idx = pick_index(SERIAL_CHARSET.len());
        *item = SERIAL_CHARSET[idx] as u16;
    }
    serial[len] = 0;
}

/// Génère un numéro de série qui imite le format et la longueur exacte de l'original.
/// Si l'original est un placeholder (ex: `"Default string"`), génère un serial propre
/// de la **même longueur** (`orig_len`) pour remplacer la chaîne SMBIOS in-place sans padding.
pub fn generate_serial_matching_format(serial: &mut [u16; 64], original: &[u16]) {
    let orig_len = str_len_u16(original);
    if orig_len == 0 {
        generate_serial(serial, 14);
        return;
    }

    let len = orig_len.min(63);

    // Si c'est un placeholder ("Default string", "To be filled by O.E.M."),
    // on génère un vrai serial alphanumérique de la même longueur exacte
    // pour ne pas garder les espaces et les points du placeholder.
    if is_placeholder(original) {
        generate_serial(serial, len);
        return;
    }

    let is_hex = is_likely_hex_serial(original);

    for i in 0..len {
        let ch = (original[i] & 0xFF) as u8;

        if ch.is_ascii_digit() {
            let charset: &[u8] = if i == 0 && ch != b'0' {
                DIGITS_NO_ZERO
            } else {
                DIGITS
            };
            serial[i] = charset[pick_index(charset.len())] as u16;
        } else if ch.is_ascii_uppercase() {
            let charset: &[u8] = if is_hex && ch.is_ascii_hexdigit() {
                HEX_UPPER
            } else {
                UPPER_LETTERS
            };
            serial[i] = charset[pick_index(charset.len())] as u16;
        } else if ch.is_ascii_lowercase() {
            serial[i] = LOWER_LETTERS[pick_index(LOWER_LETTERS.len())] as u16;
        } else {
            // Conserver les séparateurs ('-', '.', '/')
            serial[i] = original[i];
        }
    }

    // Garantir que la valeur générée diffère de l'originale
    if slice_eq_u16(&serial[..len], &original[..len]) {
        for i in 0..len {
            let ch = (original[i] & 0xFF) as u8;
            if ch.is_ascii_digit() {
                serial[i] = if ch == b'9' { b'8' as u16 } else { (ch + 1) as u16 };
                break;
            } else if ch.is_ascii_uppercase() {
                serial[i] = if ch == b'Z' { b'Y' as u16 } else { (ch + 1) as u16 };
                break;
            } else if ch.is_ascii_lowercase() {
                serial[i] = if ch == b'z' { b'y' as u16 } else { (ch + 1) as u16 };
                break;
            }
        }
    }

    serial[len] = 0;
}

// ─── Génération réaliste par constructeur ───────────────────────────────────

/// Patterns de numéros de série par constructeur : `(nom, longueur_défaut, charset, préfixe)`.
const VENDOR_PATTERNS: &[(&str, usize, &[u8], &[u8])] = &[
    // 0: Dell : Service Tag 7 caractères alphanumériques uppercase (ex: "ABC1234")
    ("Dell", 7, b"123456789ABCDEFGHJKMNPQRSTUVWXYZ", b""),
    // 1: Lenovo : préfixe "PF" + 8 caractères (ex: "PF12345678")
    ("Lenovo", 10, b"0123456789ABCDEFGHJKLMNPQRSTUVWXYZ", b"PF"),
    // 2: HP : 10 caractères alphanumériques (ex: "5CG1234ABC")
    ("HP", 10, b"123456789ABCDEFGHJKMNPQRSTUVWXYZ", b"5CG"),
    // 3: Apple : 12 caractères alphanumériques (ex: "C02N1234ABCD")
    ("Apple", 12, b"123456789ABCDEFGHJKMNPQRSTUVWXYZ", b"C02"),
    // 4: ASUS : 12-15 caractères alphanumériques/chiffres
    ("ASUS", 12, b"0123456789ABCDEFGHJKLMNPQRSTUVWXYZ", b""),
    // 5: MSI : format "MSI" + 9 chiffres
    ("MSI", 12, b"0123456789", b"MSI"),
    // 6: Gigabyte : format "SN" + 10 chiffres
    ("Gigabyte", 12, b"0123456789", b"SN"),
    // 7: Acer : 22 caractères alphanumériques
    ("Acer", 22, b"123456789ABCDEFGHJKMNPQRSTUVWXYZ", b"NX"),
];

/// Détecte le constructeur probable à partir d'un numéro de série original.
/// Retourne l'index dans `VENDOR_PATTERNS` ou `None` si inconnu.
pub fn detect_vendor(original: &[u16]) -> Option<usize> {
    if is_placeholder(original) {
        return None;
    }
    let len = str_len_u16(original);
    if len == 0 {
        return None;
    }

    // Dell : 7 chars alphanumériques
    if len == 7 && original[..7].iter().all(|&c| ((c & 0xFF) as u8).is_ascii_alphanumeric()) {
        return Some(0);
    }
    // Lenovo : commence par "PF" ou "R9" (longueur 8..=10)
    if (8..=10).contains(&len)
        && ((original[0] == b'P' as u16 && original[1] == b'F' as u16)
            || (original[0] == b'R' as u16 && original[1] == b'9' as u16))
    {
        return Some(1);
    }
    // HP : commence par "5CG", "CND", "MXL", "CZC" (10 chars)
    if len == 10
        && (starts_with_ignore_case(original, b"5CG")
            || starts_with_ignore_case(original, b"CND")
            || starts_with_ignore_case(original, b"MXL")
            || starts_with_ignore_case(original, b"CZC"))
    {
        return Some(2);
    }
    // Apple : commence par "C0" ou "CK" (10..=12 chars)
    if (10..=12).contains(&len)
        && ((original[0] == b'C' as u16 && original[1] == b'0' as u16)
            || (original[0] == b'C' as u16 && original[1] == b'K' as u16))
    {
        return Some(3);
    }
    // MSI : commence par "MSI"
    if len >= 6 && starts_with_ignore_case(original, b"MSI") {
        return Some(5);
    }
    // Gigabyte : commence par "SN" + chiffres
    if len == 12 && starts_with_ignore_case(original, b"SN") {
        return Some(6);
    }
    // ASUS : 12-15 chiffres
    if (12..=15).contains(&len)
        && original
            .iter()
            .take(len)
            .all(|&c| ((c & 0xFF) as u8).is_ascii_digit())
    {
        return Some(4);
    }

    None
}

/// Détecte le constructeur à partir du nom de fabricant SMBIOS (Type 1 Manufacturer / Type 2 Manufacturer).
pub fn detect_vendor_by_name(manufacturer: &[u16]) -> Option<usize> {
    if contains_ignore_case(manufacturer, b"dell") || contains_ignore_case(manufacturer, b"alienware") {
        Some(0)
    } else if contains_ignore_case(manufacturer, b"lenovo") {
        Some(1)
    } else if contains_ignore_case(manufacturer, b"hp") || contains_ignore_case(manufacturer, b"hewlett") {
        Some(2)
    } else if contains_ignore_case(manufacturer, b"apple") {
        Some(3)
    } else if contains_ignore_case(manufacturer, b"asus") || contains_ignore_case(manufacturer, b"republic of gamers") {
        Some(4)
    } else if contains_ignore_case(manufacturer, b"msi") || contains_ignore_case(manufacturer, b"micro-star") {
        Some(5)
    } else if contains_ignore_case(manufacturer, b"gigabyte") {
        Some(6)
    } else if contains_ignore_case(manufacturer, b"acer") {
        Some(7)
    } else {
        None
    }
}

/// Génère un numéro de série crédible pour un constructeur donné.
pub fn generate_vendor_serial(serial: &mut [u16; 64], vendor_index: usize) {
    if vendor_index >= VENDOR_PATTERNS.len() {
        generate_serial(serial, 14);
        return;
    }

    let (_name, total_len, charset, prefix) = VENDOR_PATTERNS[vendor_index];
    generate_vendor_serial_with_len(serial, vendor_index, total_len, charset, prefix);
}

fn generate_vendor_serial_with_len(
    serial: &mut [u16; 64],
    _vendor_index: usize,
    target_len: usize,
    charset: &[u8],
    prefix: &[u8],
) {
    let total = target_len.clamp(1, 63);
    let prefix_len = if total > prefix.len() { prefix.len() } else { 0 };

    for (i, &b) in prefix.iter().take(prefix_len).enumerate() {
        serial[i] = b as u16;
    }

    for item in serial.iter_mut().take(total).skip(prefix_len) {
        *item = charset[pick_index(charset.len())] as u16;
    }

    serial[total] = 0;
}

/// Génère un serial intelligent qui respecte la longueur de l'original en mémoire
/// tout en appliquant les règles du constructeur détecté.
pub fn generate_serial_smart(serial: &mut [u16; 64], original: &[u16]) {
    let orig_len = str_len_u16(original);
    if let Some(vendor_idx) = detect_vendor(original) {
        let (_name, default_len, charset, prefix) = VENDOR_PATTERNS[vendor_idx];
        let target_len = if orig_len > 0 { orig_len } else { default_len };
        generate_vendor_serial_with_len(serial, vendor_idx, target_len, charset, prefix);
    } else {
        generate_serial_matching_format(serial, original);
    }
}

// ─── Génération MAC Address ──────────────────────────────────────────────────

/// Génère une adresse MAC aléatoire unicast localement administrée (`x2:xx:xx:xx:xx:xx`).
pub fn generate_mac_address(mac: &mut [u8; 6]) {
    random_bytes(mac);
    mac[0] = (mac[0] & 0xFC) | 0x02;
}

/// Formate une adresse MAC en chaîne UTF-16 `"XX:XX:XX:XX:XX:XX"`.
pub fn format_mac(mac: &[u8; 6], buf: &mut [u16; 18]) {
    const HEX: &[u8] = b"0123456789ABCDEF";
    let mut pos = 0;
    for (i, &byte) in mac.iter().enumerate() {
        if i > 0 {
            buf[pos] = b':' as u16;
            pos += 1;
        }
        buf[pos] = HEX[(byte >> 4) as usize] as u16;
        pos += 1;
        buf[pos] = HEX[(byte & 0x0F) as usize] as u16;
        pos += 1;
    }
    buf[17] = 0;
}

// ─── Génération BIOS & Hardware Strings ──────────────────────────────────────

/// Liste des vendors BIOS réalistes.
const BIOS_VENDORS: &[&str] = &[
    "American Megatrends Inc.",
    "American Megatrends International, LLC.",
    "Phoenix Technologies Ltd.",
    "INSYDE Corp.",
    "Insyde Software Corp.",
    "Award Software International Inc.",
];

/// Génère un nom de vendor BIOS aléatoire.
pub fn generate_bios_vendor(buf: &mut [u16; 64]) {
    let idx = pick_index(BIOS_VENDORS.len());
    let vendor = BIOS_VENDORS[idx];
    for (i, &b) in vendor.as_bytes().iter().enumerate() {
        if i >= 63 {
            break;
        }
        buf[i] = b as u16;
    }
    let end = vendor.len().min(63);
    buf[end] = 0;
}

/// Génère ou conserve un vendor BIOS compatible avec la longueur originale.
/// Si un original existe, on conserve le nom du vendor du firmware (modifier "ASUS" ou "AMI"
/// avec une chaîne de taille différente tronquerait le nom) ou on imite son format.
pub fn generate_bios_vendor_matching(buf: &mut [u16; 64], original: &[u16]) {
    let orig_len = str_len_u16(original);
    if orig_len == 0 {
        generate_bios_vendor(buf);
        return;
    }
    // Pour le BIOS Vendor (ex: "American Megatrends Inc."), chercher un vendor de même longueur
    // ou conserver le préfixe et varier la révision si pertinent, sinon garder le vendor légitime.
    for &candidate in BIOS_VENDORS {
        if candidate.len() == orig_len {
            let mut same = true;
            for (i, &b) in candidate.as_bytes().iter().enumerate() {
                if original[i] != b as u16 {
                    same = false;
                    break;
                }
            }
            if !same {
                for (i, &b) in candidate.as_bytes().iter().enumerate() {
                    buf[i] = b as u16;
                }
                buf[orig_len] = 0;
                return;
            }
        }
    }
    // Sinon conserver le vendor original intact (évite de tronquer "American Megatrends Inc.")
    str_copy_u16(buf, original);
}

fn write_u16_d2(buf: &mut [u16; 64], pos: &mut usize, val: u8) {
    if *pos + 2 > 62 {
        return;
    }
    buf[*pos] = (b'0' + (val / 10)) as u16;
    *pos += 1;
    buf[*pos] = (b'0' + (val % 10)) as u16;
    *pos += 1;
}

fn write_u16_u32(buf: &mut [u16; 64], pos: &mut usize, val: u32) {
    if val == 0 {
        if *pos < 62 {
            buf[*pos] = b'0' as u16;
            *pos += 1;
        }
        return;
    }
    let mut tmp = [0u8; 10];
    let mut i = 0;
    let mut v = val;
    while v > 0 && i < 10 {
        tmp[i] = (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    let mut j = i;
    while j > 0 && *pos < 62 {
        j -= 1;
        buf[*pos] = (b'0' + tmp[j]) as u16;
        *pos += 1;
    }
}

/// Génère une version de BIOS réaliste (ex: `"2.20.1271"`, `"F.23"`, `"P.14"`).
pub fn generate_bios_version(buf: &mut [u16; 64]) {
    let format_idx = pick_index(3);
    let mut pos: usize = 0;

    match format_idx {
        0 => {
            let maj = ((random_u32() % 5) + 1) as u8;
            buf[pos] = (b'0' + maj) as u16;
            pos += 1;
            buf[pos] = b'.' as u16;
            pos += 1;
            let min = (random_u32() % 30) as u8;
            write_u16_d2(buf, &mut pos, min);
            buf[pos] = b'.' as u16;
            pos += 1;
            let rev = 1000 + (random_u32() % 4000);
            write_u16_u32(buf, &mut pos, rev);
        }
        1 => {
            let prefix = if (random_u32() & 1) == 0 { b'F' } else { b'A' };
            buf[pos] = prefix as u16;
            pos += 1;
            buf[pos] = b'.' as u16;
            pos += 1;
            let ver = 10 + (random_u32() % 89) as u8;
            write_u16_d2(buf, &mut pos, ver);
        }
        _ => {
            let prefix = b"PNR"[pick_index(3)];
            buf[pos] = prefix as u16;
            pos += 1;
            buf[pos] = b'.' as u16;
            pos += 1;
            let ver = 10 + (random_u32() % 89) as u8;
            write_u16_d2(buf, &mut pos, ver);
        }
    }

    buf[pos.min(63)] = 0;
}

/// Génère une version de BIOS qui respecte exactement la structure et la longueur de l'originale
/// (ex: `"FX506LH.310"` -> `"FX506LH.314"`, `"1.20.0"` -> `"1.24.2"`).
pub fn generate_bios_version_matching(buf: &mut [u16; 64], original: &[u16]) {
    let orig_len = str_len_u16(original);
    if orig_len == 0 || is_placeholder(original) {
        generate_bios_version(buf);
        return;
    }
    let len = orig_len.min(63);
    str_copy_u16(buf, original);

    // Si la version contient un point (ex: "FX506LH.310"), ne modifier que les chiffres après le point
    let dot_pos = original[..len].iter().rposition(|&c| c == b'.' as u16);
    let start_idx = dot_pos.map(|p| p + 1).unwrap_or(0);

    let mut changed = false;
    for i in start_idx..len {
        let ch = (original[i] & 0xFF) as u8;
        if ch.is_ascii_digit() {
            let new_digit = b'0' + (pick_index(10) as u8);
            buf[i] = new_digit as u16;
            if buf[i] != original[i] {
                changed = true;
            }
        }
    }

    if !changed {
        generate_serial_matching_format(buf, original);
    }
    buf[len] = 0;
}

/// Génère une date de release BIOS réaliste au format `"MM/DD/YYYY"` (10 caractères).
pub fn generate_bios_date(buf: &mut [u16; 64]) {
    let month = 1 + (random_u32() % 12);
    let day = 1 + (random_u32() % 28);
    let year = 2020 + (random_u32() % 6);

    let mut pos: usize = 0;
    write_u16_d2(buf, &mut pos, month as u8);
    buf[pos] = b'/' as u16;
    pos += 1;
    write_u16_d2(buf, &mut pos, day as u8);
    buf[pos] = b'/' as u16;
    pos += 1;
    write_u16_u32(buf, &mut pos, year);
    buf[pos.min(63)] = 0;
}

/// Génère une date BIOS de même longueur que l'originale (`"MM/DD/YYYY"` ou `"MM/DD/YY"`).
pub fn generate_bios_date_matching(buf: &mut [u16; 64], original: &[u16]) {
    let orig_len = str_len_u16(original);
    if orig_len == 8 && original[2] == b'/' as u16 && original[5] == b'/' as u16 {
        // Format court "MM/DD/YY"
        let month = (1 + (random_u32() % 12)) as u8;
        let day = (1 + (random_u32() % 28)) as u8;
        let year_short = (20 + (random_u32() % 6)) as u8;
        let mut pos = 0;
        write_u16_d2(buf, &mut pos, month);
        buf[pos] = b'/' as u16;
        pos += 1;
        write_u16_d2(buf, &mut pos, day);
        buf[pos] = b'/' as u16;
        pos += 1;
        write_u16_d2(buf, &mut pos, year_short);
        buf[pos] = 0;
    } else if orig_len == 10 || orig_len == 0 {
        generate_bios_date(buf);
    } else {
        generate_serial_matching_format(buf, original);
    }
}

/// Alias pratique pour `str_len_u16`.
#[inline]
pub fn str_len(s: &[u16]) -> usize {
    str_len_u16(s)
}

/// Génère un numéro de série pour un constructeur donné avec longueur de repli.
pub fn generate_serial_for_vendor(buf: &mut [u16; 64], vendor_idx: usize, fallback_len: usize) {
    generate_vendor_serial(buf, vendor_idx);
    if buf[0] == 0 {
        generate_serial(buf, fallback_len);
    }
}

/// Alias vers `generate_bios_version_matching`.
#[inline]
pub fn generate_bios_version_like(buf: &mut [u16; 64], original: &[u16]) {
    generate_bios_version_matching(buf, original);
}

/// Alias vers `generate_bios_date_matching`.
#[inline]
pub fn generate_bios_date_like(buf: &mut [u16; 64], original: &[u16]) {
    generate_bios_date_matching(buf, original);
}

/// Génère un numéro de série de barrette RAM (8 caractères hexadécimaux par défaut, ou format-matched).
pub fn generate_ram_serial_matching(buf: &mut [u16; 64], original: &[u16]) {
    let orig_len = str_len_u16(original);
    if orig_len == 0 {
        for item in buf.iter_mut().take(8) {
            *item = HEX_UPPER[pick_index(HEX_UPPER.len())] as u16;
        }
        buf[8] = 0;
        return;
    }
    if is_placeholder(original) {
        let len = orig_len.min(63);
        for item in buf.iter_mut().take(len) {
            *item = HEX_UPPER[pick_index(HEX_UPPER.len())] as u16;
        }
        buf[len] = 0;
        return;
    }
    generate_serial_matching_format(buf, original);
}

/// Génère un numéro de série de barrette RAM déterministe par slot (`seed` + `slot_index`),
/// afin que chaque barrette RAM garde le même numéro de série d'un boot à l'autre.
pub fn generate_ram_serial_deterministic(
    buf: &mut [u16; 64],
    original: &[u16],
    seed: u32,
    slot_index: u32,
) {
    let mut state = seed
        .wrapping_add(slot_index.wrapping_mul(0x9E37_79B9))
        .wrapping_add(0x85EB_CA6B);
    if state == 0 {
        state = 0x5241_4D30;
    }
    let next_u32 = |s: &mut u32| -> u32 {
        *s ^= *s << 13;
        *s ^= *s >> 17;
        *s ^= *s << 5;
        *s
    };

    let orig_len = str_len_u16(original);
    let len = if orig_len == 0 { 8 } else { orig_len.min(63) };

    if orig_len == 0 || is_placeholder(original) || is_likely_hex_serial(original) {
        for item in buf.iter_mut().take(len) {
            let r = (next_u32(&mut state) as usize) % HEX_UPPER.len();
            *item = HEX_UPPER[r] as u16;
        }
        buf[len] = 0;
        return;
    }

    for i in 0..len {
        let ch = original[i];
        let b = (ch & 0xFF) as u8;
        let r = next_u32(&mut state) as usize;
        if b.is_ascii_digit() {
            buf[i] = DIGITS[r % DIGITS.len()] as u16;
        } else if b.is_ascii_uppercase() {
            buf[i] = UPPER_LETTERS[r % UPPER_LETTERS.len()] as u16;
        } else if b.is_ascii_lowercase() {
            buf[i] = LOWER_LETTERS[r % LOWER_LETTERS.len()] as u16;
        } else {
            buf[i] = ch;
        }
    }
    buf[len] = 0;
}

/// Génère un numéro de série de disque (NVMe / SATA / ATA) en respectant la longueur
/// et le format du numéro de série original (jusqu'à 20 caractères pour ATA/NVMe standard),
/// ou un numéro de série alphanumérique réaliste de 15 caractères si l'original est vide.
pub fn generate_disk_serial_matching(buf: &mut [u16; 64], original: &[u16]) {
    buf.fill(0);
    let orig_len = str_len_u16(original);
    if orig_len == 0 || is_placeholder(original) {
        let len = if orig_len == 0 { 15 } else { orig_len.min(20) };
        generate_serial(buf, len);
        return;
    }
    let capped_len = orig_len.min(20);
    let mut trimmed_orig = [0u16; 64];
    trimmed_orig[..capped_len].copy_from_slice(&original[..capped_len]);
    generate_serial_matching_format(buf, &trimmed_orig);
}

/// Génère un numéro de série de disque déterministe par disque (`seed` + `disk_index`),
/// afin que chaque disque physique conserve un numéro de série stable d'un boot à l'autre
/// tout en respectant le masque de caractères du disque d'origine (ex: `S4EVNF0M123456A` ou `ABCD_1234_5678_90AB.`).
pub fn generate_disk_serial_deterministic(
    buf: &mut [u16; 64],
    original: &[u16],
    seed: u32,
    disk_index: u32,
) {
    buf.fill(0);
    let mut state = seed
        .wrapping_add(disk_index.wrapping_mul(0x9E37_79B9))
        .wrapping_add(0xD15C_5EED);
    if state == 0 {
        state = 0x4E56_4D45;
    }
    let next_u32 = |s: &mut u32| -> u32 {
        *s ^= *s << 13;
        *s ^= *s >> 17;
        *s ^= *s << 5;
        *s
    };

    let orig_len = str_len_u16(original);
    let len = if orig_len == 0 { 15 } else { orig_len.min(20) };

    if orig_len == 0 || is_placeholder(original) {
        for item in buf.iter_mut().take(len) {
            let r = (next_u32(&mut state) as usize) % SERIAL_CHARSET.len();
            *item = SERIAL_CHARSET[r] as u16;
        }
        buf[len] = 0;
        return;
    }

    if is_likely_hex_serial(&original[..len]) {
        for i in 0..len {
            let ch = original[i];
            let b = (ch & 0xFF) as u8;
            if b.is_ascii_hexdigit() {
                let r = (next_u32(&mut state) as usize) % HEX_UPPER.len();
                buf[i] = HEX_UPPER[r] as u16;
            } else {
                buf[i] = ch;
            }
        }
        buf[len] = 0;
        return;
    }

    for i in 0..len {
        let ch = original[i];
        let b = (ch & 0xFF) as u8;
        let r = next_u32(&mut state) as usize;
        if b.is_ascii_digit() {
            buf[i] = DIGITS[r % DIGITS.len()] as u16;
        } else if b.is_ascii_uppercase() {
            buf[i] = UPPER_LETTERS[r % UPPER_LETTERS.len()] as u16;
        } else if b.is_ascii_lowercase() {
            buf[i] = LOWER_LETTERS[r % LOWER_LETTERS.len()] as u16;
        } else {
            // Préserve les séparateurs de format NVMe/EUI (`_`, `-`, `.`)
            buf[i] = ch;
        }
    }
    buf[len] = 0;
}

// ─── Délai UEFI ─────────────────────────────────────────────────────────────

/// Attend un certain nombre de microsecondes via l'UEFI `Stall()`.
#[cfg(target_os = "uefi")]
#[allow(dead_code)]
pub fn delay_us(microseconds: u64) {
    uefi::boot::stall(Duration::from_micros(microseconds));
}

// ─── Tests unitaires ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_v4_version_byte() {
        let mut uuid = [0u8; 16];
        generate_uuid(&mut uuid);
        assert_eq!(uuid[6] & 0xF0, 0x40, "UUID version bits should be 0100");
        assert_eq!(uuid[8] & 0xC0, 0x80, "UUID variant bits should be 10");
    }

    #[test]
    fn test_uuid_to_smbios_mixed_endian() {
        let uuid: [u8; 16] = [
            0x01, 0x02, 0x03, 0x04, // time_low
            0x05, 0x06,             // time_mid
            0x07, 0x08,             // time_hi
            0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
        ];
        let smbios = uuid_to_smbios_bytes(&uuid);
        assert_eq!(&smbios[0..4], &[0x04, 0x03, 0x02, 0x01]);
        assert_eq!(&smbios[4..6], &[0x06, 0x05]);
        assert_eq!(&smbios[6..8], &[0x08, 0x07]);
        assert_eq!(&smbios[8..16], &uuid[8..16]);
    }

    #[test]
    fn test_generate_serial_length() {
        let mut serial = [0u16; 64];
        generate_serial(&mut serial, 10);
        assert_eq!(str_len_u16(&serial), 10);
        assert_eq!(serial[10], 0);
    }

    #[test]
    fn test_generate_serial_uniqueness() {
        let mut a = [0u16; 64];
        let mut b = [0u16; 64];
        random_set_seed(42);
        generate_serial(&mut a, 14);
        random_set_seed(9999);
        generate_serial(&mut b, 14);
        assert!(!slice_eq_u16(&a, &b));
    }

    #[test]
    fn test_placeholder_detection() {
        let mut ph = [0u16; 64];
        for (i, &b) in b"To be filled by O.E.M.".iter().enumerate() {
            ph[i] = b as u16;
        }
        assert!(is_placeholder(&ph));

        let mut real = [0u16; 64];
        for (i, &b) in b"PF2A9K81".iter().enumerate() {
            real[i] = b as u16;
        }
        assert!(!is_placeholder(&real));
    }

    #[test]
    fn test_placeholder_replaced_without_spaces() {
        let mut ph = [0u16; 64];
        for (i, &b) in b"Default string".iter().enumerate() {
            ph[i] = b as u16;
        }
        let mut out = [0u16; 64];
        random_set_seed(123);
        generate_serial_matching_format(&mut out, &ph);
        assert_eq!(str_len_u16(&out), 14);
        assert!(!out[..14].contains(&(b' ' as u16)));
    }

    #[test]
    fn test_hex_serial_preserved() {
        let mut hex_orig = [0u16; 64];
        for (i, &b) in b"0123456789ABCDEF".iter().enumerate() {
            hex_orig[i] = b as u16;
        }
        assert!(is_likely_hex_serial(&hex_orig));

        let mut out = [0u16; 64];
        random_set_seed(777);
        generate_serial_matching_format(&mut out, &hex_orig);
        assert_eq!(str_len_u16(&out), 16);
        assert!(is_likely_hex_serial(&out));
    }

    #[test]
    fn test_detect_vendor_dell_and_lenovo() {
        let mut dell = [0u16; 64];
        for (i, &b) in b"ABC1234".iter().enumerate() {
            dell[i] = b as u16;
        }
        assert_eq!(detect_vendor(&dell), Some(0));

        let mut lenovo = [0u16; 64];
        for (i, &b) in b"PF12345678".iter().enumerate() {
            lenovo[i] = b as u16;
        }
        assert_eq!(detect_vendor(&lenovo), Some(1));
    }

    #[test]
    fn test_generate_mac_locally_administered() {
        let mut mac = [0u8; 6];
        random_set_seed(42);
        generate_mac_address(&mut mac);
        assert_eq!(mac[0] & 0x02, 0x02);
        assert_eq!(mac[0] & 0x01, 0x00);
    }

    #[test]
    fn test_generate_bios_date_format() {
        let mut buf = [0u16; 64];
        random_set_seed(42);
        generate_bios_date(&mut buf);
        assert_eq!(str_len_u16(&buf), 10);
        assert_eq!(buf[2], b'/' as u16);
        assert_eq!(buf[5], b'/' as u16);
    }
}
