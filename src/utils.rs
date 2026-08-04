//! Utilitaires : génération aléatoire, format UUID, délais
//!
//! ## Ce que tu apprends ici :
//! - `static mut` : variable globale mutable (dangereux, mais nécessaire en bare-metal)
//! - `unsafe` : pourquoi on en a besoin et comment le justifier
//! - LCG (Linear Congruential Generator) : le générateur aléatoire le plus simple
//! - Manipulation de tableaux de bytes

#[cfg(target_os = "uefi")]
use core::time::Duration;

// ─── Générateur aléatoire (LCG) ───────────────────────────────────────────

/// Graine du générateur LCG.
/// `static mut` est nécessaire car on n'a pas de heap en UEFI.
/// On documente toujours POURQUOI c'est safe :
/// SAFETY: on est en single-thread en UEFI (pas de concurrence),
/// donc pas de data race possible sur cette variable.
static mut RANDOM_SEED: u32 = 0;

/// Initialise la graine avec l'horloge système UEFI.
/// C'est l'équivalent Rust de la fonction `EfiRandom()` dans `persistence.c`.
/// Note : `get_next_monotonic_count()` n'existe plus en uefi 0.39,
/// on utilise uniquement `get_time()` pour l'entropie.
/// Stub pour les tests host — initialise avec une graine fixe.
#[cfg(not(target_os = "uefi"))]
pub fn random_init() {
    unsafe { RANDOM_SEED = 0xDEAD_BEEF; }
}

#[cfg(target_os = "uefi")]
pub fn random_init() {
    let mut seed: u32 = 0;

    // On essaie d'obtenir l'heure pour l'entropie
    if let Ok(time) = uefi::runtime::get_time() {
        seed = (time.year() as u32).wrapping_mul(10000)
            .wrapping_add(time.month() as u32 * 100)
            .wrapping_add(time.day() as u32);
        seed ^= (time.hour() as u32)
            .wrapping_mul(3600)
            .wrapping_add(time.minute() as u32 * 60)
            .wrapping_add(time.second() as u32);
        seed ^= time.nanosecond();
    }

    // Fallback si tout échoue
    if seed == 0 {
        seed = 0xDEAD_BEEF;
    }

    unsafe {
        RANDOM_SEED = seed;
    }
}

/// Générateur LCG simple. Retourne un u32 pseudo-aléatoire.
/// Formule : seed = seed * 1103515245 + 12345 (LCG de glibc)
pub fn random_u32() -> u32 {
    unsafe {
        RANDOM_SEED = RANDOM_SEED.wrapping_mul(1103515245).wrapping_add(12345);
        RANDOM_SEED
    }
}

/// Remplit un buffer avec des bytes aléatoires
pub fn random_bytes(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        *byte = (random_u32() & 0xFF) as u8;
    }
}

// ─── UUID ──────────────────────────────────────────────────────────────────

/// Génère un UUID v4 (RFC 4122) aléatoire.
/// Un UUID fait 16 bytes. On génère tout aléatoirement puis on fixe :
/// - byte 6 : bits 0-3 mis à 0100 (version 4)
/// - byte 8 : bits 0-1 mis à 10 (variant RFC 4122)
pub fn generate_uuid(uuid: &mut [u8; 16]) {
    random_bytes(uuid);
    uuid[6] = (uuid[6] & 0x0F) | 0x40; // Version 4
    uuid[8] = (uuid[8] & 0x3F) | 0x80; // Variant RFC 4122
}

/// Formate un UUID en chaîne UEFI (UTF-16).
/// Format : "XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"
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
}

// ─── Génération de numéros de série ────────────────────────────────────────

/// Jeu de caractères pour les numéros de série aléatoires.
/// On évite les caractères ambigus (O/0, I/1, L) pour la lisibilité.
const SERIAL_CHARSET: &[u8] = b"123456789ABCDEFGHJKMNPQRSTUVWXYZ";
const DIGITS: &[u8] = b"0123456789";
const DIGITS_NO_ZERO: &[u8] = b"123456789";
const UPPER_LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
const HEX_UPPER: &[u8] = b"0123456789ABCDEF";

/// Génère un numéro de série aléatoire de longueur donnée.
/// Équivalent de `EfiGenerateRandomSerial` en C.
pub fn generate_serial(serial: &mut [u16; 64], length: usize) {
    let len = length.min(63);
    for i in 0..len {
        let idx = (random_u32() as usize) % SERIAL_CHARSET.len();
        serial[i] = SERIAL_CHARSET[idx] as u16;
    }
    serial[len] = 0; // Null terminator UTF-16
}

/// Génère un numéro de série qui imite le format de l'original.
/// Garde la même longueur et les mêmes classes de caractères.
/// C'est l'équivalent Rust de `EfiGenerateRandomSerialMatchingFormat`.
pub fn generate_serial_matching_format(
    serial: &mut [u16; 64],
    original: &[u16],
) {
    let orig_len = str_len_u16(original);
    if orig_len == 0 {
        // Pas d'original, on génère un format par défaut
        generate_serial(serial, 14);
        return;
    }

    let len = orig_len.min(63);

    // Analyse du format de l'original
    for i in 0..len {
        let ch = original[i] as u32;

        if (b'0' as u32..=b'9' as u32).contains(&ch) {
            // Chiffre → remplacer par un autre chiffre
            let charset: &[u8] = if i == 0 && ch != b'0' as u32 {
                DIGITS_NO_ZERO
            } else {
                DIGITS
            };
            let idx = (random_u32() as usize) % charset.len();
            serial[i] = charset[idx] as u16;
        } else if (b'A' as u32..=b'Z' as u32).contains(&ch) || (b'a' as u32..=b'z' as u32).contains(&ch) {
            // Lettre → remplacer par une autre lettre
            let idx = (random_u32() as usize) % UPPER_LETTERS.len();
            serial[i] = UPPER_LETTERS[idx] as u16;
        } else {
            // Séparateur ou caractère spécial → garder tel quel
            serial[i] = original[i];
        }
    }

    // S'assurer que le résultat est différent de l'original
    if slice_eq_u16(&serial[..len], &original[..len]) {
        // Modifier un caractère pour garantir la différence
        for i in 0..len {
            if (b'0' as u32..=b'9' as u32).contains(&(original[i] as u32)) {
                serial[i] = if original[i] == b'9' as u16 {
                    b'8' as u16
                } else {
                    original[i] + 1
                };
                break;
            }
        }
    }

    serial[len] = 0;
}

// ─── Utilitaires chaînes UTF-16 ────────────────────────────────────────────

/// Longueur d'une chaîne UTF-16 terminée par null.
pub fn str_len_u16(s: &[u16]) -> usize {
    s.iter().position(|&c| c == 0).unwrap_or(s.len())
}

/// Copie une chaîne UTF-16.
pub fn str_copy_u16(dst: &mut [u16], src: &[u16]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d = *s;
        if *s == 0 {
            break;
        }
    }
}

/// Compare deux slices u16 (équivalent de memcmp).
pub fn slice_eq_u16(a: &[u16], b: &[u16]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

/// Vérifie si un numéro de série est une valeur placeholder
/// (genre "Default string", "To be filled by O.E.M.", etc.)
pub fn is_placeholder(serial: &[u16]) -> bool {
    let s: &[u16] = serial;
    let len = str_len_u16(s);
    if len == 0 {
        return true;
    }
    // Liste des placeholders courants
    let placeholders: &[&[u16]] = &[
        &string_to_u16::<16>("DEFAULT STRING"),
        &string_to_u16::<16>("Default string"),
        &string_to_u16::<16>("default string"),
        &string_to_u16::<32>("To be filled by O.E.M."),
        &string_to_u16::<32>("To Be Filled By O.E.M."),
        &string_to_u16::<32>("System Serial Number"),
        &string_to_u16::<32>("System Product Name"),
        &string_to_u16::<16>("Not Specified"),
        &string_to_u16::<16>("Unknown"),
    ];
    for ph in placeholders {
        if slice_eq_u16(&s[..ph.len().min(len)], ph) && ph.len() == len {
            return true;
        }
    }
    false
}

/// Convertit une string Rust en tableau u16 terminé par null (pour UEFI).
const fn string_to_u16<const N: usize>(s: &str) -> [u16; N] {
    let bytes = s.as_bytes();
    let mut result = [0u16; N];
    let mut i = 0;
    while i < bytes.len() && i < N - 1 {
        result[i] = bytes[i] as u16;
        i += 1;
    }
    result
}

// ─── Génération réaliste par constructeur ───────────────────────────────────

/// Patterns de numéros de série par constructeur.
/// Chaque entrée : (préfixe, longueur totale, charset, segments).
/// Ex: Dell = 7 lettres, Lenovo = "PF" + 8 chiffres, HP = 8 lettres+chiffres.
const VENDOR_PATTERNS: &[(&str, usize, &[u8], &[u8])] = &[
    // Dell : 7 caractères alphanumériques uppercase (ex: "ABC1234")
    ("Dell", 7, b"123456789ABCDEFGHJKMNPQRSTUVWXYZ", b""),
    // Lenovo : préfixe "PF" + 8 chiffres (ex: "PF12345678")
    ("Lenovo", 10, b"0123456789", b"PF"),
    // HP : 10 caractères, 3 lettres + 3 chiffres + 4 aléatoires
    ("HP", 10, b"123456789ABCDEFGHJKMNPQRSTUVWXYZ", b""),
    // Apple : 12 caractères alphanumériques (ex: "C02N1234ABCD")
    ("Apple", 12, b"123456789ABCDEFGHJKMNPQRSTUVWXYZ", b"C02"),
    // ASUS : 12 chiffres (ex: "123456789012")
    ("ASUS", 12, b"0123456789", b""),
    // MSI : format "MSI" + 9 chiffres
    ("MSI", 12, b"0123456789", b"MSI"),
    // Gigabyte : format "SN" + 10 chiffres
    ("Gigabyte", 12, b"0123456789", b"SN"),
    // Acer : 22 caractères alphanumériques
    ("Acer", 22, b"123456789ABCDEFGHJKMNPQRSTUVWXYZ", b""),
];

/// Détecte le constructeur probable à partir d'un numéro de série original.
/// Retourne l'index dans `VENDOR_PATTERNS` ou `None` si inconnu.
pub fn detect_vendor(original: &[u16]) -> Option<usize> {
    let s = string_to_u16_array(original);
    let len = str_len_u16(&s);

    if len == 0 {
        return None;
    }

    // Heuristiques courantes
    // Dell : 7 chars alphanumériques uppercase
    if len == 7 {
        return Some(0);
    }
    // Lenovo : commence par "PF" ou "R9"
    if len == 10 && (s[0] == b'P' as u16 && s[1] == b'F' as u16)
        || (s[0] == b'R' as u16 && s[1] == b'9' as u16)
    {
        return Some(1);
    }
    // Apple : commence par "C0" ou "CK"
    if (len == 12 || len == 11) && (s[0] == b'C' as u16 && s[1] == b'0' as u16)
        || (s[0] == b'C' as u16 && s[1] == b'K' as u16)
    {
        return Some(3);
    }
    // MSI : commence par "MSI"
    if len >= 3 && s[0] == b'M' as u16 && s[1] == b'S' as u16 && s[2] == b'I' as u16 {
        return Some(5);
    }
    // ASUS : 12-15 chiffres
    if (len >= 12 && len <= 15) && s.iter().take(len).all(|&c| (c as u8).is_ascii_digit()) {
        return Some(4);
    }

    None
}

/// Génère un numéro de série crédible pour un constructeur donné.
/// `vendor_index` = index dans VENDOR_PATTERNS.
pub fn generate_vendor_serial(serial: &mut [u16; 64], vendor_index: usize) {
    if vendor_index >= VENDOR_PATTERNS.len() {
        generate_serial(serial, 14);
        return;
    }

    let (_name, total_len, charset, prefix) = VENDOR_PATTERNS[vendor_index];
    let prefix_len = prefix.len();
    let total = total_len.min(63);

    // Copier le préfixe
    for (i, &b) in prefix.iter().enumerate() {
        serial[i] = b as u16;
    }

    // Générer le reste aléatoirement
    for i in prefix_len..total {
        let idx = (random_u32() as usize) % charset.len();
        serial[i] = charset[idx] as u16;
    }

    serial[total] = 0;
}

/// Génère un serial format-matched, avec détection automatique du constructeur
/// si l'original correspond à un pattern connu.
pub fn generate_serial_smart(serial: &mut [u16; 64], original: &[u16]) {
    if let Some(vendor_idx) = detect_vendor(original) {
        generate_vendor_serial(serial, vendor_idx);
    } else {
        // Fallback : format-matched générique
        generate_serial_matching_format(serial, original);
    }
}

/// Convertit une slice u16 en array u16 pour analyse.
fn string_to_u16_array(src: &[u16]) -> [u16; 64] {
    let mut arr = [0u16; 64];
    let len = src.len().min(64);
    arr[..len].copy_from_slice(&src[..len]);
    arr
}

// ─── Couleurs console UEFI ─────────────────────────────────────────────────

/// Attributs de couleur pour la console UEFI.
/// Format UEFI : fond sur 4 bits hauts, texte sur 4 bits bas.
pub const COLOR_DEFAULT: usize = 0x07;   // Gris clair sur noir
pub const COLOR_HEADER: usize = 0x1A;    // Vert clair sur bleu
pub const COLOR_IMPORTANT: usize = 0x0E; // Jaune sur noir
pub const COLOR_OK: usize = 0x0B;        // Cyan sur noir
pub const COLOR_FAIL: usize = 0x0C;      // Rouge clair sur noir

// ─── Délai ─────────────────────────────────────────────────────────────────

/// Attend un certain nombre de microsecondes via l'UEFI Stall().
#[cfg(target_os = "uefi")]
pub fn delay_us(microseconds: u64) {
    uefi::boot::stall(Duration::from_micros(microseconds));
}

// ─── Tests unitaires ────────────────────────────────────────────────────────
// Exécution : cargo test --target x86_64-pc-windows-msvc

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_v4_version_byte() {
        let mut uuid = [0u8; 16];
        generate_uuid(&mut uuid);
        // Version 4 : byte 6 bits 0-3 = 0100
        assert_eq!(uuid[6] & 0xF0, 0x40, "UUID version bits should be 0100");
        // Variant RFC 4122 : byte 8 bits 0-1 = 10
        assert_eq!(uuid[8] & 0xC0, 0x80, "UUID variant bits should be 10");
    }

    #[test]
    fn test_generate_serial_length() {
        let mut serial = [0u16; 64];
        generate_serial(&mut serial, 10);
        assert_eq!(str_len_u16(&serial), 10, "Serial should be 10 chars");
        assert!(serial[10] == 0, "Position 10 should be null terminator");
    }

    #[test]
    fn test_generate_serial_uniqueness() {
        let mut a = [0u16; 64];
        let mut b = [0u16; 64];
        // Initialiser le random avec des graines différentes
        unsafe { RANDOM_SEED = 42; }
        generate_serial(&mut a, 14);
        unsafe { RANDOM_SEED = 9999; }
        generate_serial(&mut b, 14);
        assert!(!slice_eq_u16(&a, &b), "Two serials should differ");
    }

    #[test]
    fn test_str_len_u16() {
        let s: [u16; 4] = [65, 66, 0, 67];
        assert_eq!(str_len_u16(&s), 2);
    }

    #[test]
    fn test_str_len_u16_empty() {
        let s: [u16; 2] = [0, 0];
        assert_eq!(str_len_u16(&s), 0);
    }

    #[test]
    fn test_slice_eq_u16_true() {
        let a: [u16; 3] = [1, 2, 3];
        let b: [u16; 3] = [1, 2, 3];
        assert!(slice_eq_u16(&a, &b));
    }

    #[test]
    fn test_slice_eq_u16_false() {
        let a: [u16; 3] = [1, 2, 3];
        let b: [u16; 3] = [1, 2, 4];
        assert!(!slice_eq_u16(&a, &b));
    }

    #[test]
    fn test_format_uuid() {
        let uuid: [u8; 16] = [
            0x55, 0x0E, 0x84, 0x00, 0xE2, 0x9B, 0x41, 0xD4,
            0xA7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00,
        ];
        let mut buf = [0u16; 37];
        format_uuid(&uuid, &mut buf);
        // Vérifier les tirets aux bonnes positions
        assert_eq!(buf[8], b'-' as u16);
        assert_eq!(buf[13], b'-' as u16);
        assert_eq!(buf[18], b'-' as u16);
        assert_eq!(buf[23], b'-' as u16);
    }

    #[test]
    fn test_generate_serial_matching_format_keeps_length() {
        let orig: [u16; 64] = {
            let mut a = [0u16; 64];
            a[0] = b'A' as u16; a[1] = b'B' as u16; a[2] = b'1' as u16;
            a[3] = b'2' as u16; a[4] = b'3' as u16; a[5] = 0;
            a
        };
        let mut serial = [0u16; 64];
        unsafe { RANDOM_SEED = 12345; }
        generate_serial_matching_format(&mut serial, &orig);
        assert_eq!(str_len_u16(&serial), 5, "New serial should keep same length");
        // Ne doit pas être identique à l'original
        assert!(!slice_eq_u16(&serial, &orig), "New serial must differ from original");
    }

    #[test]
    fn test_detect_vendor_dell() {
        let s: [u16; 64] = {
            let mut a = [0u16; 64];
            for (i, c) in b"ABC1234".iter().enumerate() {
                a[i] = *c as u16;
            }
            a
        };
        assert_eq!(detect_vendor(&s), Some(0)); // Dell
    }

    #[test]
    fn test_detect_vendor_lenovo() {
        let s: [u16; 64] = {
            let mut a = [0u16; 64];
            for (i, c) in b"PF12345678".iter().enumerate() {
                a[i] = *c as u16;
            }
            a
        };
        assert_eq!(detect_vendor(&s), Some(1)); // Lenovo
    }

    #[test]
    fn test_generate_vendor_serial_dell() {
        let mut serial = [0u16; 64];
        unsafe { RANDOM_SEED = 42; }
        generate_vendor_serial(&mut serial, 0); // Dell = 7 chars
        assert_eq!(str_len_u16(&serial), 7);
    }

    #[test]
    fn test_generate_vendor_serial_lenovo() {
        let mut serial = [0u16; 64];
        unsafe { RANDOM_SEED = 42; }
        generate_vendor_serial(&mut serial, 1); // Lenovo = "PF" prefix
        assert_eq!(str_len_u16(&serial), 10);
        assert_eq!(serial[0], b'P' as u16);
        assert_eq!(serial[1], b'F' as u16);
    }

    #[test]
    fn test_generate_serial_smart_fallsback() {
        // Un format inconnu → doit utiliser matching_format (pas paniquer)
        let orig: [u16; 64] = {
            let mut a = [0u16; 64];
            for (i, c) in b"XYZ-9999-ABC".iter().enumerate() {
                a[i] = *c as u16;
            }
            a
        };
        let mut serial = [0u16; 64];
        unsafe { RANDOM_SEED = 42; }
        generate_serial_smart(&mut serial, &orig);
        assert_eq!(str_len_u16(&serial), 12);
        // Les séparateurs doivent être préservés
        assert_eq!(serial[3], b'-' as u16);
        assert_eq!(serial[8], b'-' as u16);
    }
}
