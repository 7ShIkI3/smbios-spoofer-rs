//! Recherche de l'Entry Point SMBIOS dans le firmware.
//!
//! ## Ce que tu apprends ici :
//! - Parcourir la table de configuration UEFI via `with_config_table()`
//! - Chercher par GUID (identifiant unique de 128 bits)
//! - HOB List (Hand-Off Block) — méthode alternative de discovery PI/DXE
//! - Option<T> vs pointeur null : la différence de philosophie C vs Rust
//! - Vérification de checksum (intégrité des données)

use crate::smbios;
use uefi::Guid;

// ─── GUIDs pour la HOB List ─────────────────────────────────────────────────

/// GUID de la HOB List dans la Config Table.
/// La HOB List contient les blocs passés de PEI à DXE.
const HOB_LIST_GUID: Guid = Guid::new(
    [0x4C, 0x9F, 0x73, 0x77], // time_low: 0x7739f24c
    [0xD7, 0x93],              // time_mid: 0x93d7
    [0xD4, 0x11],              // time_high_and_version: 0x11d4
    0x9A,                      // clock_seq_high_and_reserved
    0x3A,                      // clock_seq_low
    [0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D], // node
);

// ─── Structures HOB ─────────────────────────────────────────────────────────

/// Header commun à tous les HOBs.
#[repr(C)]
struct HobHeader {
    hob_type: u16,
    hob_length: u16,
}

/// HOB de type GUID Extension (type 0x0003).
/// Contient un GUID suivi de données arbitraires.
#[repr(C)]
struct HobGuidExtension {
    header: HobHeader,
    guid: Guid,
    // data: [u8; header.hob_length - size_of::<HobGuidExtension>()]
}

// Constantes HOB
const HOB_TYPE_END_OF_LIST: u16 = 0xFFFF;
const HOB_TYPE_GUID_EXTENSION: u16 = 0x0003;

// ─── Vérification du checksum ───────────────────────────────────────────────

/// Vérifie le checksum du SMBIOS Entry Point (somme des bytes = 0).
/// Équivalent de `CheckEntry()` en C.
fn check_entry(entry: &smbios::SmbiosEntry) -> bool {
    if &entry.anchor_string != b"_SM_" {
        return false;
    }
    if entry.length == 0 || entry.length > 64 {
        return false;
    }
    let ptr = entry as *const smbios::SmbiosEntry as *const u8;
    let mut sum: u8 = 0;
    for i in 0..entry.length as usize {
        sum = sum.wrapping_add(unsafe { *ptr.add(i) });
    }
    sum == 0
}

/// Extrait un SmbiosEntry depuis une adresse mémoire.
/// SAFETY: `ptr` doit pointer vers une structure SmbiosEntry valide.
unsafe fn entry_from_ptr(ptr: *const smbios::SmbiosEntry) -> Option<&'static smbios::SmbiosEntry> {
    if ptr.is_null() {
        return None;
    }
    let entry = unsafe { &*ptr };
    if check_entry(entry) {
        Some(entry)
    } else {
        None
    }
}

// ─── Méthode 1 : Config Table ───────────────────────────────────────────────

/// Cherche le SMBIOS Entry Point via la table de configuration UEFI.
fn find_via_config_table() -> Option<&'static smbios::SmbiosEntry> {
    uefi::system::with_config_table(|entries| {
        for entry in entries {
            if entry.guid == smbios::SMBIOS_TABLE_GUID
                || entry.guid == smbios::SMBIOS3_TABLE_GUID
            {
                let ptr = entry.address as *const smbios::SmbiosEntry;
                if let Some(result) = unsafe { entry_from_ptr(ptr) } {
                    return Some(result);
                }
            }
        }
        None
    })
}

// ─── Méthode 2 : HOB List (fallback) ────────────────────────────────────────

/// Parcourt la HOB List pour trouver un GUID Extension HOB contenant
/// les tables SMBIOS. Utilisé en fallback quand la Config Table ne contient
/// pas les GUIDs SMBIOS (rare, mais possible sur certains firmwares).
fn find_via_hob_list() -> Option<&'static smbios::SmbiosEntry> {
    // 1. Trouver la HOB List dans la Config Table
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

    // 2. Parcourir les HOBs
    let mut current = hob_list_addr;
    loop {
        // SAFETY: le firmware garantit que la HOB List est en mémoire valide
        let header = unsafe { &*current.cast::<HobHeader>() };

        // Fin de la HOB list ?
        if header.hob_type == HOB_TYPE_END_OF_LIST {
            break;
        }

        // HOB GUID Extension ?
        if header.hob_type == HOB_TYPE_GUID_EXTENSION {
            // SAFETY: le header est valide, on peut lire le GUID extension
            let hob_guid_ext = unsafe { &*current.cast::<HobGuidExtension>() };

            // Ce HOB contient-il les tables SMBIOS ?
            if hob_guid_ext.guid == smbios::SMBIOS_TABLE_GUID
                || hob_guid_ext.guid == smbios::SMBIOS3_TABLE_GUID
            {
                // Les données commencent juste après le GUID
                let data_ptr = unsafe {
                    current
                        .add(core::mem::size_of::<HobGuidExtension>())
                        .cast::<*const smbios::SmbiosEntry>()
                        .read()
                };
                if let Some(result) = unsafe { entry_from_ptr(data_ptr) } {
                    return Some(result);
                }
            }
        }

        // HOB suivant
        current = unsafe { current.add(header.hob_length as usize) };
    }

    None
}

// ─── API publique ────────────────────────────────────────────────────────────

/// Recherche l'Entry Point SMBIOS. Essaie dans l'ordre :
///   1. Table de configuration UEFI (méthode standard)
///   2. HOB List (fallback — firmwares PI/DXE)
///
/// Équivalent de `FindByConfig()` + `FindEntry()` + HOB fallback en C.
pub fn find_smbios_entry() -> Option<&'static smbios::SmbiosEntry> {
    find_via_config_table()
        .or_else(find_via_hob_list)
}

/// Version simplifiée : trouve l'adresse brute de la table.
pub fn find_smbios_entry_raw() -> Option<*const u8> {
    let entry = find_smbios_entry()?;
    Some(entry.structure_table_address as *const u8)
}
