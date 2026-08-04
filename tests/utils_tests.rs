/// Tests unitaires pour les fonctions pures de utils.rs.
/// Ces tests compilent sur le host (pas besoin d'UEFI).
///
/// Exécution : cargo test --target x86_64-pc-windows-msvc --tests

// On inclut le module utils directement
#[path = "../src/utils.rs"]
mod utils;

use utils::*;

#[test]
fn test_uuid_v4_has_correct_version() {
    let mut uuid = [0u8; 16];
    generate_uuid(&mut uuid);
    // Version 4 : byte 6 bits 0-3 = 0100
    assert_eq!(uuid[6] & 0xF0, 0x40);
    // Variant RFC 4122 : byte 8 bits 0-1 = 10
    assert_eq!(uuid[8] & 0xC0, 0x80);
}

#[test]
fn test_generate_serial_correct_length() {
    let mut serial = [0u16; 64];
    generate_serial(&mut serial, 10);
    assert_eq!(str_len_u16(&serial), 10);
}

#[test]
fn test_serials_differ_with_different_seeds() {
    let mut a = [0u16; 64];
    let mut b = [0u16; 64];
    unsafe { RANDOM_SEED = 42; }
    generate_serial(&mut a, 14);
    unsafe { RANDOM_SEED = 9999; }
    generate_serial(&mut b, 14);
    assert!(!slice_eq_u16(&a, &b));
}

#[test]
fn test_str_len_u16_stops_at_null() {
    let s: [u16; 4] = [65, 66, 0, 67];
    assert_eq!(str_len_u16(&s), 2);
}

#[test]
fn test_format_uuid_has_correct_dashes() {
    let uuid: [u8; 16] = [
        0x55, 0x0E, 0x84, 0x00, 0xE2, 0x9B, 0x41, 0xD4,
        0xA7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00,
    ];
    let mut buf = [0u16; 37];
    format_uuid(&uuid, &mut buf);
    assert_eq!(buf[8], b'-' as u16);
    assert_eq!(buf[13], b'-' as u16);
    assert_eq!(buf[18], b'-' as u16);
    assert_eq!(buf[23], b'-' as u16);
}

#[test]
fn test_matching_format_keeps_same_length() {
    let orig: [u16; 64] = {
        let mut a = [0u16; 64];
        for (i, c) in b"ABC12345".iter().enumerate() {
            a[i] = *c as u16;
        }
        a
    };
    let mut serial = [0u16; 64];
    unsafe { RANDOM_SEED = 12345; }
    generate_serial_matching_format(&mut serial, &orig);
    assert_eq!(str_len_u16(&serial), 8);
    assert!(!slice_eq_u16(&serial, &orig));
}

#[test]
fn test_detect_vendor_dell_7chars() {
    let s = make_test_serial(b"ABC1234");
    assert_eq!(detect_vendor(&s), Some(0));
}

#[test]
fn test_detect_vendor_lenovo_pf_prefix() {
    let s = make_test_serial(b"PF12345678");
    assert_eq!(detect_vendor(&s), Some(1));
}

#[test]
fn test_detect_vendor_apple_c0_prefix() {
    let s = make_test_serial(b"C02N1234ABCD");
    assert_eq!(detect_vendor(&s), Some(3));
}

#[test]
fn test_generate_vendor_lenovo_keeps_pf_prefix() {
    let mut serial = [0u16; 64];
    unsafe { RANDOM_SEED = 42; }
    generate_vendor_serial(&mut serial, 1);
    assert_eq!(str_len_u16(&serial), 10);
    assert_eq!(serial[0], b'P' as u16);
    assert_eq!(serial[1], b'F' as u16);
}

#[test]
fn test_smart_generation_falls_back_for_unknown() {
    let orig = make_test_serial(b"XYZ-9999-ABC");
    let mut serial = [0u16; 64];
    unsafe { RANDOM_SEED = 42; }
    generate_serial_smart(&mut serial, &orig);
    assert_eq!(str_len_u16(&serial), 12);
}

// Helper
fn make_test_serial(bytes: &[u8]) -> [u16; 64] {
    let mut a = [0u16; 64];
    for (i, c) in bytes.iter().enumerate() {
        a[i] = *c as u16;
    }
    a
}
