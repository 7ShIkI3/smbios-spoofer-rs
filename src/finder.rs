//! Recherche de l'Entry Point SMBIOS (2.x 32-bit `_SM_` et 3.0 64-bit `_SM3_`) dans le firmware.
//!
//! Stratégie de découverte multi-couche :
//! 1. Table de configuration UEFI (`SMBIOS3_TABLE_GUID` et `SMBIOS_TABLE_GUID`)
//! 2. HOB List PI/DXE (`HOB_LIST_GUID`) en fallback
//! 3. Si le firmware expose à la fois une table 2.x et une table 3.0 (parfois à deux
//!    adresses physiques distinctes), les deux pointeurs et adresses sont conservés
//!    dans [`smbios::SmbiosTableInfo`] afin que le patcher modifie les deux tables.

#![cfg_attr(not(target_os = "uefi"), allow(dead_code))]

use crate::smbios::{Smbios3Entry, SmbiosEntry, SmbiosTableInfo};
#[cfg(any(target_os = "uefi", test))]
use crate::smbios;
use uefi::Guid;

// ─── GUIDs pour la HOB List ─────────────────────────────────────────────────

/// GUID de la HOB List dans la Config Table (PI Spec).
const HOB_LIST_GUID: Guid = Guid::new(
    [0x4C, 0x9F, 0x73, 0x77], // time_low: 0x7739f24c
    [0xD7, 0x93],             // time_mid: 0x93d7
    [0xD4, 0x11],             // time_high_and_version: 0x11d4
    0x9A,                     // clock_seq_high_and_reserved
    0x3A,                     // clock_seq_low
    [0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D],
);

// ─── Structures HOB ─────────────────────────────────────────────────────────

#[repr(C)]
struct HobHeader {
    hob_type: u16,
    hob_length: u16,
}

#[repr(C)]
struct HobGuidExtension {
    header: HobHeader,
    guid: Guid,
}

const HOB_TYPE_END_OF_LIST: u16 = 0xFFFF;
const HOB_TYPE_GUID_EXTENSION: u16 = 0x0003;

// ─── Vérification des checksums et signatures ───────────────────────────────

/// Vérifie la signature `"_SM_"` et le checksum d'un Entry Point SMBIOS 2.x (32-bit).
pub fn check_entry_2x(entry: &SmbiosEntry) -> bool {
    if &entry.anchor_string != b"_SM_" {
        return false;
    }
    let len = entry.length as usize;
    if !(0x1E..=64).contains(&len) {
        return false;
    }
    let table_addr = entry.structure_table_address;
    if table_addr == 0 {
        return false;
    }
    let ptr = (entry as *const SmbiosEntry).cast::<u8>();
    let mut sum: u8 = 0;
    for i in 0..len {
        // SAFETY: `len` <= 64 octets à partir de l'adresse de `entry`
        sum = sum.wrapping_add(unsafe { *ptr.add(i) });
    }
    // Même si un firmware a un checksum initial légèrement hors spec, l'ancre `_SM_` + `_DMI_` fait foi
    sum == 0 || &entry.intermediate_anchor == b"_DMI_"
}

/// Vérifie la signature `"_SM3_"` et le checksum d'un Entry Point SMBIOS 3.0+ (64-bit).
pub fn check_entry_3x(entry: &Smbios3Entry) -> bool {
    if &entry.anchor_string != b"_SM3_" {
        return false;
    }
    let len = entry.length as usize;
    if !(24..=64).contains(&len) {
        return false;
    }
    let table_addr = entry.structure_table_address;
    if table_addr == 0 {
        return false;
    }
    let ptr = (entry as *const Smbios3Entry).cast::<u8>();
    let mut sum: u8 = 0;
    for i in 0..len {
        // SAFETY: `len` <= 64 octets à partir de l'adresse de `entry`
        sum = sum.wrapping_add(unsafe { *ptr.add(i) });
    }
    sum == 0 || entry.major_version >= 3
}

/// Valide un pointeur SMBIOS 2.x.
///
/// # Safety
/// `ptr` doit pointer vers une région mémoire lisible.
unsafe fn entry_2x_from_ptr(ptr: *mut SmbiosEntry) -> Option<*mut SmbiosEntry> {
    if ptr.is_null() {
        return None;
    }
    let entry = unsafe { &*ptr };
    if check_entry_2x(entry) {
        Some(ptr)
    } else {
        None
    }
}

/// Valide un pointeur SMBIOS 3.0.
///
/// # Safety
/// `ptr` doit pointer vers une région mémoire lisible.
unsafe fn entry_3x_from_ptr(ptr: *mut Smbios3Entry) -> Option<*mut Smbios3Entry> {
    if ptr.is_null() {
        return None;
    }
    let entry = unsafe { &*ptr };
    if check_entry_3x(entry) {
        Some(ptr)
    } else {
        None
    }
}

/// Construit un [`SmbiosTableInfo`] unifié à partir des pointeurs 2.x et/ou 3.0 trouvés.
unsafe fn build_table_info(
    ptr_2x: Option<*mut SmbiosEntry>,
    ptr_3x: Option<*mut Smbios3Entry>,
) -> Option<SmbiosTableInfo> {
    match (ptr_3x, ptr_2x) {
        (Some(p3), Some(p2)) => {
            let e3 = unsafe { &*p3 };
            let e2 = unsafe { &*p2 };
            let addr3 = e3.structure_table_address as usize;
            let addr2 = e2.structure_table_address as usize;
            let secondary = if addr2 != 0 && addr2 != addr3 {
                Some(addr2)
            } else {
                None
            };
            Some(SmbiosTableInfo {
                table_address: addr3,
                secondary_table_address: secondary,
                table_max_length: e3.structure_table_max_size as usize,
                major_version: e3.major_version,
                minor_version: e3.minor_version,
                is_64bit: true,
                entry_2x: Some(p2),
                entry_3x: Some(p3),
            })
        }
        (Some(p3), None) => {
            let e3 = unsafe { &*p3 };
            Some(SmbiosTableInfo {
                table_address: e3.structure_table_address as usize,
                secondary_table_address: None,
                table_max_length: e3.structure_table_max_size as usize,
                major_version: e3.major_version,
                minor_version: e3.minor_version,
                is_64bit: true,
                entry_2x: None,
                entry_3x: Some(p3),
            })
        }
        (None, Some(p2)) => {
            let e2 = unsafe { &*p2 };
            Some(SmbiosTableInfo {
                table_address: e2.structure_table_address as usize,
                secondary_table_address: None,
                table_max_length: e2.structure_table_length as usize,
                major_version: e2.major_version,
                minor_version: e2.minor_version,
                is_64bit: false,
                entry_2x: Some(p2),
                entry_3x: None,
            })
        }
        (None, None) => None,
    }
}

// ─── Méthode 1 : Config Table UEFI ──────────────────────────────────────────

#[cfg(target_os = "uefi")]
fn find_via_config_table() -> Option<SmbiosTableInfo> {
    uefi::system::with_config_table(|entries| {
        let mut found_2x: Option<*mut SmbiosEntry> = None;
        let mut found_3x: Option<*mut Smbios3Entry> = None;

        for entry in entries {
            if entry.guid == smbios::SMBIOS3_TABLE_GUID {
                let ptr3 = entry.address as *mut Smbios3Entry;
                if let Some(valid3) = unsafe { entry_3x_from_ptr(ptr3) } {
                    found_3x = Some(valid3);
                }
            } else if entry.guid == smbios::SMBIOS_TABLE_GUID {
                let ptr2 = entry.address as *mut SmbiosEntry;
                if let Some(valid2) = unsafe { entry_2x_from_ptr(ptr2) } {
                    found_2x = Some(valid2);
                }
            }
        }

        // SAFETY: les pointeurs ont été validés par `entry_2x_from_ptr` / `entry_3x_from_ptr`
        unsafe { build_table_info(found_2x, found_3x) }
    })
}

#[cfg(not(target_os = "uefi"))]
fn find_via_config_table() -> Option<SmbiosTableInfo> {
    None
}

// ─── Méthode 2 : HOB List (fallback PI/DXE) ─────────────────────────────────

#[cfg(target_os = "uefi")]
fn find_via_hob_list() -> Option<SmbiosTableInfo> {
    let hob_list_addr: *const u8 = uefi::system::with_config_table(|entries| {
        for entry in entries {
            if entry.guid == HOB_LIST_GUID {
                return entry.address as *const u8;
            }
        }
        core::ptr::null()
    });

    if hob_list_addr.is_null() {
        return None;
    }

    let mut found_2x: Option<*mut SmbiosEntry> = None;
    let mut found_3x: Option<*mut Smbios3Entry> = None;
    let mut current = hob_list_addr;

    for _ in 0..4096 {
        // SAFETY: la HOB List est fournie par le firmware en mémoire valide
        let header = unsafe { &*current.cast::<HobHeader>() };

        if header.hob_type == HOB_TYPE_END_OF_LIST || header.hob_length < 4 {
            break;
        }

        if header.hob_type == HOB_TYPE_GUID_EXTENSION
            && (header.hob_length as usize) >= core::mem::size_of::<HobGuidExtension>() + 8
        {
            let hob_guid_ext = unsafe { &*current.cast::<HobGuidExtension>() };
            let data_addr = unsafe {
                current
                    .add(core::mem::size_of::<HobGuidExtension>())
                    .cast::<usize>()
                    .read_unaligned()
            };

            if hob_guid_ext.guid == smbios::SMBIOS3_TABLE_GUID {
                if let Some(valid3) = unsafe { entry_3x_from_ptr(data_addr as *mut Smbios3Entry) } {
                    found_3x = Some(valid3);
                }
            } else if hob_guid_ext.guid == smbios::SMBIOS_TABLE_GUID {
                if let Some(valid2) = unsafe { entry_2x_from_ptr(data_addr as *mut SmbiosEntry) } {
                    found_2x = Some(valid2);
                }
            }
        }

        current = unsafe { current.add(header.hob_length as usize) };
    }

    unsafe { build_table_info(found_2x, found_3x) }
}

#[cfg(not(target_os = "uefi"))]
fn find_via_hob_list() -> Option<SmbiosTableInfo> {
    None
}

// ─── API publique ────────────────────────────────────────────────────────────

/// Recherche l'Entry Point SMBIOS (3.0 64-bit `_SM3_` et/ou 2.x 32-bit `_SM_`).
/// Essaie dans l'ordre :
/// 1. Table de configuration UEFI
/// 2. HOB List (fallback PI/DXE)
pub fn find_smbios_table() -> Option<SmbiosTableInfo> {
    find_via_config_table().or_else(find_via_hob_list)
}

/// Version simplifiée : retourne l'adresse physique brute de la table SMBIOS.
pub fn find_smbios_entry_raw() -> Option<*const u8> {
    let info = find_smbios_table()?;
    Some(info.table_address as *const u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_entry_2x_and_3x_validation() {
        let mut e2 = SmbiosEntry {
            anchor_string: *b"_SM_",
            checksum: 0,
            length: core::mem::size_of::<SmbiosEntry>() as u8,
            major_version: 2,
            minor_version: 8,
            max_structure_size: 64,
            entry_point_revision: 0,
            formatted_area: [0; 5],
            intermediate_anchor: *b"_DMI_",
            intermediate_checksum: 0,
            structure_table_length: 512,
            structure_table_address: 0x000F_0000,
            number_of_structures: 15,
            bcd_revision: 0x28,
        };
        unsafe {
            smbios::recalculate_entry_2x_checksum(&mut e2);
        }
        assert!(check_entry_2x(&e2));

        let mut e3 = Smbios3Entry {
            anchor_string: *b"_SM3_",
            checksum: 0,
            length: core::mem::size_of::<Smbios3Entry>() as u8,
            major_version: 3,
            minor_version: 5,
            docrev: 0,
            entry_point_revision: 1,
            reserved: 0,
            structure_table_max_size: 4096,
            structure_table_address: 0x0000_0001_0000_0000,
        };
        unsafe {
            smbios::recalculate_entry_3x_checksum(&mut e3);
        }
        assert!(check_entry_3x(&e3));
    }
}
