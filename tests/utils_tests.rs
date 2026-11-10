//! Tests d'intégration et unitaires pour les fonctions pures de `utils.rs`.
//! Ces tests compilent et s'exécutent sur l'hôte (`cargo test --target x86_64-pc-windows-msvc`).

#![allow(dead_code)]

#[path = "../src/utils.rs"]
mod utils;

use utils::*;

#[test]
fn test_uuid_v4_has_correct_version() {
    let mut uuid = [0u8; 16];
    generate_uuid(&mut uuid);
    assert_eq!(uuid[6] & 0xF0, 0x40);
    assert_eq!(uuid[8] & 0xC0, 0x80);
}

#[test]
fn test_uuid_to_smbios_bytes_roundtrip() {
    let orig: [u8; 16] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
        0xFF,
    ];
    let wire = uuid_to_smbios_bytes(&orig);
    assert_eq!(wire[0..4], [0x33, 0x22, 0x11, 0x00]);
    assert_eq!(wire[4..6], [0x55, 0x44]);
    assert_eq!(wire[6..8], [0x77, 0x66]);
    assert_eq!(wire[8..16], orig[8..16]);
    // Involution : f(f(x)) == x
    assert_eq!(uuid_to_smbios_bytes(&wire), orig);
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
    random_set_seed(42);
    generate_serial(&mut a, 14);
    random_set_seed(9999);
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
        0x55, 0x0E, 0x84, 0x00, 0xE2, 0x9B, 0x41, 0xD4, 0xA7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00,
        0x00,
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
    let orig = make_test_serial(b"ABC12345");
    let mut serial = [0u16; 64];
    random_set_seed(12345);
    generate_serial_matching_format(&mut serial, &orig);
    assert_eq!(str_len_u16(&serial), 8);
    assert!(!slice_eq_u16(&serial, &orig));
}

#[test]
fn test_placeholder_detection_and_replacement() {
    let placeholder = make_test_serial(b"To Be Filled By O.E.M.");
    assert!(is_placeholder(&placeholder));
    let mut out = [0u16; 64];
    random_set_seed(777);
    generate_serial_matching_format(&mut out, &placeholder);
    assert_eq!(str_len_u16(&out), 22);
    assert!(!is_placeholder(&out));
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
fn test_detect_vendor_by_name() {
    let asus = make_test_serial(b"ASUSTeK COMPUTER INC.");
    assert_eq!(detect_vendor_by_name(&asus), Some(4));
    let msi = make_test_serial(b"Micro-Star International Co., Ltd.");
    assert_eq!(detect_vendor_by_name(&msi), Some(5));
}

#[test]
fn test_generate_vendor_lenovo_keeps_pf_prefix() {
    let mut serial = [0u16; 64];
    random_set_seed(42);
    generate_vendor_serial(&mut serial, 1);
    assert_eq!(str_len_u16(&serial), 10);
    assert_eq!(serial[0], b'P' as u16);
    assert_eq!(serial[1], b'F' as u16);
}

#[test]
fn test_smart_generation_falls_back_for_unknown() {
    let orig = make_test_serial(b"XYZ-9999-ABC");
    let mut serial = [0u16; 64];
    random_set_seed(42);
    generate_serial_smart(&mut serial, &orig);
    assert_eq!(str_len_u16(&serial), 12);
}

#[test]
fn test_ram_serial_deterministic() {
    let orig = make_test_serial(b"1A2B3C4D");
    let mut s0_a = [0u16; 64];
    let mut s0_b = [0u16; 64];
    let mut s1 = [0u16; 64];
    generate_ram_serial_deterministic(&mut s0_a, &orig, 0x12345678, 0);
    generate_ram_serial_deterministic(&mut s0_b, &orig, 0x12345678, 0);
    generate_ram_serial_deterministic(&mut s1, &orig, 0x12345678, 1);
    assert!(slice_eq_u16(&s0_a, &s0_b));
    assert!(!slice_eq_u16(&s0_a, &s1));
    assert_eq!(str_len_u16(&s0_a), 8);
}

#[test]
fn test_disk_serial_deterministic_and_format() {
    let orig = make_test_serial(b"0000_0000_0100_0000_4CE0_0019_51C2_5D02.");
    let mut d0_a = [0u16; 64];
    let mut d0_b = [0u16; 64];
    let mut d1 = [0u16; 64];
    generate_disk_serial_deterministic(&mut d0_a, &orig, 0xCAFEBABE, 0);
    generate_disk_serial_deterministic(&mut d0_b, &orig, 0xCAFEBABE, 0);
    generate_disk_serial_deterministic(&mut d1, &orig, 0xCAFEBABE, 1);
    assert!(slice_eq_u16(&d0_a, &d0_b));
    assert!(!slice_eq_u16(&d0_a, &d1));
    // Capped at 20 chars (standard ATA/NVMe Identify SN length) and underscores preserved
    assert_eq!(str_len_u16(&d0_a), 20);
    assert_eq!(d0_a[4], b'_' as u16);
    assert_eq!(d0_a[9], b'_' as u16);
}

fn make_test_serial(bytes: &[u8]) -> [u16; 64] {
    let mut a = [0u16; 64];
    for (i, c) in bytes.iter().enumerate() {
        a[i] = *c as u16;
    }
    a
}
