//! Lecture et patching en place des tables SMBIOS (2.x 32-bit et 3.0 64-bit).
//!
//! Couvre les structures :
//! - **Type 0** (BIOS Information) : Vendor, Version, Release Date
//! - **Type 1** (System Information) : UUID (mixed-endian SMBIOS 2.6+), Serial Number, SKU Number
//! - **Type 2** (Baseboard Information) : Serial Number, Asset Tag
//! - **Type 3** (System Enclosure / Chassis) : Serial Number, Asset Tag
//! - **Type 4** (Processor Information) : Serial Number, Asset Tag, Part Number (tous les sockets)
//! - **Type 11** (OEM Strings) : Randomisation déterministe basée sur `config.oem_seed`
//! - **Type 17** (Memory Device / RAM DIMM) : Serial Number et Asset Tag de chaque barrette RAM
//!   peuplée (`size > 0`), générés de façon déterministe à partir de `config.ram_serial_seed`

use crate::persistence::SpoofConfig;
use crate::smbios::{self, SmbiosTableInfo};
use crate::utils;

// ─── Structures de rapport et de sauvegarde des originaux ──────────────────

/// Photographie complète des identifiants SMBIOS matériels originaux
/// capturée **avant** tout patch en mémoire.
#[derive(Clone, Copy)]
pub struct OriginalSmbiosData {
    pub uuid: [u8; 16],
    pub system_manufacturer: [u16; 64],
    pub system_product: [u16; 64],
    pub system_serial: [u16; 64],
    pub system_sku: [u16; 64],
    pub baseboard_manufacturer: [u16; 64],
    pub baseboard_model: [u16; 64],
    pub baseboard_serial: [u16; 64],
    pub baseboard_asset_tag: [u16; 64],
    pub chassis_serial: [u16; 64],
    pub chassis_asset_tag: [u16; 64],
    pub processor_serial: [u16; 64],
    pub bios_vendor: [u16; 64],
    pub bios_version: [u16; 64],
    pub bios_date: [u16; 64],
    pub ram_dimm_count: usize,
    pub first_ram_serial: [u16; 64],
    pub first_ram_part: [u16; 64],
    pub disk_count: usize,
    pub first_disk_serial: [u16; 64],
    pub first_disk_model: [u16; 64],
}

impl Default for OriginalSmbiosData {
    fn default() -> Self {
        Self {
            uuid: [0u8; 16],
            system_manufacturer: [0u16; 64],
            system_product: [0u16; 64],
            system_serial: [0u16; 64],
            system_sku: [0u16; 64],
            baseboard_manufacturer: [0u16; 64],
            baseboard_model: [0u16; 64],
            baseboard_serial: [0u16; 64],
            baseboard_asset_tag: [0u16; 64],
            chassis_serial: [0u16; 64],
            chassis_asset_tag: [0u16; 64],
            processor_serial: [0u16; 64],
            bios_vendor: [0u16; 64],
            bios_version: [0u16; 64],
            bios_date: [0u16; 64],
            ram_dimm_count: 0,
            first_ram_serial: [0u16; 64],
            first_ram_part: [0u16; 64],
            disk_count: 0,
            first_disk_serial: [0u16; 64],
            first_disk_model: [0u16; 64],
        }
    }
}

/// Bilan des structures SMBIOS effectivement modifiées lors d'un `patch_all`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PatchReport {
    pub type0_patched: bool,
    pub type1_patched: bool,
    pub type2_patched: bool,
    pub type3_patched: bool,
    pub type4_patched: usize,
    pub type11_patched: bool,
    pub type17_patched: usize,
    pub protocol_synced: bool,
}

// ─── Helpers internes ──────────────────────────────────────────────────────

/// Convertit une chaîne UTF-16 null-terminated en slice ASCII exact (sans octets nuls).
fn u16_to_ascii_slice<'a>(src: &[u16], dst: &'a mut [u8; 64]) -> &'a [u8] {
    let mut len = 0usize;
    for &ch in src.iter() {
        if ch == 0 || len >= 63 {
            break;
        }
        dst[len] = if (0x20..=0x7E).contains(&ch) {
            ch as u8
        } else {
            b'X'
        };
        len += 1;
    }
    dst[len] = 0;
    &dst[..len]
}

/// Édite un champ chaîne d'une structure SMBIOS et synchronise optionnellement
/// via `EFI_SMBIOS_PROTOCOL.UpdateString`.
unsafe fn edit_field_and_sync(
    table_ptr: *mut u8,
    header_len: usize,
    handle: u16,
    field_index: u8,
    new_u16: &[u16],
) -> bool {
    if field_index == 0 || new_u16.is_empty() || new_u16[0] == 0 {
        return false;
    }
    let mut ascii_buf = [0u8; 64];
    let slice = u16_to_ascii_slice(new_u16, &mut ascii_buf);
    if slice.is_empty() {
        return false;
    }
    unsafe {
        smbios::edit_string(table_ptr, header_len, field_index, slice);
    }
    let _ = smbios::update_string_via_protocol(handle, field_index, slice);
    true
}

// ─── Patching unitaire par type SMBIOS (sur une adresse de table) ──────────

/// Patche le Type 0 (BIOS Information : Vendor, Version, Release Date).
///
/// # Safety
/// `table_address` doit pointer vers une table SMBIOS valide en mémoire.
pub unsafe fn patch_type0_at(
    table_address: usize,
    table_max_length: usize,
    config: &SpoofConfig,
) -> bool {
    let Some(table) =
        smbios::find_by_type_at(table_address, table_max_length, smbios::SMBIOS_TYPE_BIOS, 0)
    else {
        return false;
    };
    let Some(type0) = table.as_type0() else {
        return false;
    };

    let table_ptr = table.ptr.raw as *mut u8;
    let header_len = table.header_length();
    let handle = table.handle();

    let bios_vendor = config.bios_vendor;
    let bios_version = config.bios_version;
    let bios_date = config.bios_date;

    unsafe {
        // Note : on ne remplace le BIOS Vendor que s'il diffère ou est configuré
        edit_field_and_sync(table_ptr, header_len, handle, type0.vendor, &bios_vendor);
        edit_field_and_sync(
            table_ptr,
            header_len,
            handle,
            type0.bios_version,
            &bios_version,
        );
        edit_field_and_sync(
            table_ptr,
            header_len,
            handle,
            type0.bios_release_date,
            &bios_date,
        );
    }

    true
}

/// Patche le Type 1 (System Information : UUID mixed-endian, Serial Number, SKU Number).
///
/// # Safety
/// `table_address` doit pointer vers une table SMBIOS valide en mémoire.
pub unsafe fn patch_type1_at(
    table_address: usize,
    table_max_length: usize,
    config: &SpoofConfig,
) -> bool {
    let Some(table) = smbios::find_by_type_at(
        table_address,
        table_max_length,
        smbios::SMBIOS_TYPE_SYSTEM,
        0,
    ) else {
        return false;
    };
    let Some(type1) = table.as_type1() else {
        return false;
    };

    let table_ptr = table.ptr.raw as *mut u8;
    let header_len = table.header_length();
    let handle = table.handle();

    // 1. Spoof UUID au format mixed-endian SMBIOS 2.6+ (offset 0x08..0x18)
    if header_len >= 0x18 {
        let wire_uuid = utils::uuid_to_smbios_bytes(&config.uuid);
        let uuid_ptr = unsafe { table_ptr.add(8).cast::<[u8; 16]>() };
        unsafe {
            uuid_ptr.write_unaligned(wire_uuid);
        }
    }

    // 2. Spoof System Serial Number
    let system_serial = config.system_serial;
    unsafe {
        edit_field_and_sync(
            table_ptr,
            header_len,
            handle,
            type1.serial_number,
            &system_serial,
        );
    }

    // 3. Spoof System SKU Number (SMBIOS 2.4+, offset 0x19)
    if header_len >= 0x1A {
        let system_sku = config.system_sku;
        unsafe {
            edit_field_and_sync(table_ptr, header_len, handle, type1.sku_number, &system_sku);
        }
    }

    true
}

/// Patche le Type 2 (Baseboard Information : Serial Number, Asset Tag).
///
/// # Safety
/// `table_address` doit pointer vers une table SMBIOS valide en mémoire.
pub unsafe fn patch_type2_at(
    table_address: usize,
    table_max_length: usize,
    config: &SpoofConfig,
) -> bool {
    let Some(table) = smbios::find_by_type_at(
        table_address,
        table_max_length,
        smbios::SMBIOS_TYPE_BASEBOARD,
        0,
    ) else {
        return false;
    };
    let Some(type2) = table.as_type2() else {
        return false;
    };

    let table_ptr = table.ptr.raw as *mut u8;
    let header_len = table.header_length();
    let handle = table.handle();

    let baseboard_serial = config.baseboard_serial;
    let baseboard_asset_tag = config.baseboard_asset_tag;

    unsafe {
        edit_field_and_sync(
            table_ptr,
            header_len,
            handle,
            type2.serial_number,
            &baseboard_serial,
        );
        if header_len >= 0x09 {
            edit_field_and_sync(
                table_ptr,
                header_len,
                handle,
                type2.asset_tag,
                &baseboard_asset_tag,
            );
        }
    }

    true
}

/// Patche le Type 3 (System Enclosure / Chassis : Serial Number, Asset Tag).
///
/// # Safety
/// `table_address` doit pointer vers une table SMBIOS valide en mémoire.
pub unsafe fn patch_type3_at(
    table_address: usize,
    table_max_length: usize,
    config: &SpoofConfig,
) -> bool {
    let Some(table) = smbios::find_by_type_at(
        table_address,
        table_max_length,
        smbios::SMBIOS_TYPE_ENCLOSURE,
        0,
    ) else {
        return false;
    };
    let Some(type3) = table.as_type3() else {
        return false;
    };

    let table_ptr = table.ptr.raw as *mut u8;
    let header_len = table.header_length();
    let handle = table.handle();

    let chassis_serial = config.chassis_serial;
    let chassis_asset_tag = config.chassis_asset_tag;

    unsafe {
        edit_field_and_sync(
            table_ptr,
            header_len,
            handle,
            type3.serial_number,
            &chassis_serial,
        );
        edit_field_and_sync(
            table_ptr,
            header_len,
            handle,
            type3.asset_tag,
            &chassis_asset_tag,
        );
    }

    true
}

/// Patche toutes les structures Type 4 (Processor Information : Serial, AssetTag, PartNumber).
///
/// # Safety
/// `table_address` doit pointer vers une table SMBIOS valide en mémoire.
pub unsafe fn patch_type4_at(
    table_address: usize,
    table_max_length: usize,
    config: &SpoofConfig,
) -> usize {
    let mut patched_count = 0usize;
    let processor_serial = config.processor_serial;
    let chassis_asset_tag = config.chassis_asset_tag;

    for socket_idx in 0..8 {
        let Some(table) = smbios::find_by_type_at(
            table_address,
            table_max_length,
            smbios::SMBIOS_TYPE_PROCESSOR,
            socket_idx,
        ) else {
            break;
        };
        let Some(type4) = table.as_type4() else {
            continue;
        };

        let table_ptr = table.ptr.raw as *mut u8;
        let header_len = table.header_length();
        let handle = table.handle();

        unsafe {
            edit_field_and_sync(
                table_ptr,
                header_len,
                handle,
                type4.serial_number,
                &processor_serial,
            );
            if header_len >= 0x22 {
                edit_field_and_sync(
                    table_ptr,
                    header_len,
                    handle,
                    type4.asset_tag,
                    &chassis_asset_tag,
                );
            }
        }
        patched_count += 1;
    }

    patched_count
}

/// Patche le Type 11 (OEM Strings) de façon déterministe à partir de `config.oem_seed`
/// afin que les chaînes OEM restent stables d'un boot à l'autre pour le même profil.
/// Ne modifie pas les chaînes purement génériques ("Default string", "To Be Filled...").
///
/// # Safety
/// `table_address` doit pointer vers une table SMBIOS valide en mémoire.
pub unsafe fn patch_type11_at(
    table_address: usize,
    table_max_length: usize,
    config: &SpoofConfig,
) -> bool {
    let Some(table) = smbios::find_by_type_at(
        table_address,
        table_max_length,
        smbios::SMBIOS_TYPE_OEM_STRINGS,
        0,
    ) else {
        return false;
    };
    let Some(type11) = table.as_type11() else {
        return false;
    };

    let table_ptr = table.ptr.raw as *mut u8;
    let header_len = table.header_length();
    let count = type11.count;
    let seed = if config.oem_seed != 0 {
        config.oem_seed
    } else {
        0xA5A5_5A5A
    };

    for idx in 1..=count.min(16) {
        let mut orig_u16 = [0u16; 64];
        unsafe {
            smbios::read_smbios_string(table_ptr, header_len, idx, &mut orig_u16);
        }
        if orig_u16[0] == 0 || utils::is_placeholder(&orig_u16) {
            continue;
        }

        // Générer une chaîne de remplacement déterministe qui conserve la ponctuation
        let mut state = seed.wrapping_add((idx as u32).wrapping_mul(0x9E37_79B9));
        let mut new_ascii = [0u8; 64];
        let mut len = 0usize;
        for &ch in orig_u16.iter() {
            if ch == 0 || len >= 63 {
                break;
            }
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let b = ch as u8;
            new_ascii[len] = if b.is_ascii_digit() {
                b'0' + (state % 10) as u8
            } else if b.is_ascii_uppercase() {
                b'A' + (state % 26) as u8
            } else if b.is_ascii_lowercase() {
                b'a' + (state % 26) as u8
            } else {
                b
            };
            len += 1;
        }

        if len > 0 {
            unsafe {
                smbios::edit_string(table_ptr, header_len, idx, &new_ascii[..len]);
            }
        }
    }

    true
}

/// Patche toutes les structures Type 17 (Memory Device / RAM DIMM) présentes dans la table.
/// Pour chaque barrette peuplée (`size != 0`), génère un numéro de série déterministe
/// par slot à partir de `config.ram_serial_seed` en respectant le format exact de la barrette.
///
/// # Safety
/// `table_address` doit pointer vers une table SMBIOS valide en mémoire.
pub unsafe fn patch_type17_at(
    table_address: usize,
    table_max_length: usize,
    config: &SpoofConfig,
) -> usize {
    let mut patched_dimms = 0usize;
    let seed = if config.ram_serial_seed != 0 {
        config.ram_serial_seed
    } else {
        0x5241_4D31 // "RAM1"
    };

    for slot_idx in 0..32 {
        let Some(table) = smbios::find_by_type_at(
            table_address,
            table_max_length,
            smbios::SMBIOS_TYPE_MEMORY_DEVICE,
            slot_idx,
        ) else {
            break;
        };
        let Some(type17) = table.as_type17() else {
            continue;
        };

        // Ignorer les slots DIMM vides (size == 0 ou 0xFFFF = inconnu/vide)
        let dimm_size = type17.size;
        if dimm_size == 0 || dimm_size == 0xFFFF {
            continue;
        }

        let table_ptr = table.ptr.raw as *mut u8;
        let header_len = table.header_length();
        let handle = table.handle();

        // Spoof du Serial Number de la barrette RAM
        if type17.serial_number != 0 {
            let mut orig_ram_serial = [0u16; 64];
            unsafe {
                smbios::read_smbios_string(
                    table_ptr,
                    header_len,
                    type17.serial_number,
                    &mut orig_ram_serial,
                );
            }
            if orig_ram_serial[0] != 0 {
                let mut new_ram_serial = [0u16; 64];
                utils::generate_ram_serial_deterministic(
                    &mut new_ram_serial,
                    &orig_ram_serial,
                    seed,
                    slot_idx as u32,
                );
                unsafe {
                    edit_field_and_sync(
                        table_ptr,
                        header_len,
                        handle,
                        type17.serial_number,
                        &new_ram_serial,
                    );
                }
            }
        }

        // Spoof de l'Asset Tag de la barrette RAM s'il est renseigné et non-placeholder
        if header_len >= 0x1A && type17.asset_tag != 0 {
            let mut orig_asset = [0u16; 64];
            unsafe {
                smbios::read_smbios_string(
                    table_ptr,
                    header_len,
                    type17.asset_tag,
                    &mut orig_asset,
                );
            }
            if orig_asset[0] != 0 && !utils::is_placeholder(&orig_asset) {
                let mut new_asset = [0u16; 64];
                utils::generate_ram_serial_deterministic(
                    &mut new_asset,
                    &orig_asset,
                    seed ^ 0xA55E_77A6,
                    slot_idx as u32,
                );
                unsafe {
                    edit_field_and_sync(
                        table_ptr,
                        header_len,
                        handle,
                        type17.asset_tag,
                        &new_asset,
                    );
                }
            }
        }

        patched_dimms += 1;
    }

    patched_dimms
}

// ─── API haut niveau (`SmbiosTableInfo` + compatibilité `SmbiosEntry`) ─────

/// Applique tous les patchs SMBIOS sur la table principale (et sur la table secondaire
/// si les tables 2.x et 3.0 pointent vers des adresses distinctes), synchronise
/// `EFI_SMBIOS_PROTOCOL` et recalcule les checksums des Entry Points.
///
/// # Safety
/// `info` doit décrire une ou plusieurs tables SMBIOS valides en mémoire firmware.
pub unsafe fn patch_all_info(info: &SmbiosTableInfo, config: &SpoofConfig) -> PatchReport {
    let mut report = PatchReport::default();

    unsafe {
        report.type0_patched = patch_type0_at(info.table_address, info.table_max_length, config);
        report.type1_patched = patch_type1_at(info.table_address, info.table_max_length, config);
        report.type2_patched = patch_type2_at(info.table_address, info.table_max_length, config);
        report.type3_patched = patch_type3_at(info.table_address, info.table_max_length, config);
        report.type4_patched = patch_type4_at(info.table_address, info.table_max_length, config);
        report.type11_patched = patch_type11_at(info.table_address, info.table_max_length, config);
        report.type17_patched = patch_type17_at(info.table_address, info.table_max_length, config);
    }

    // Si le firmware expose une seconde table SMBIOS distincte (ex. 32-bit vs 64-bit), la patcher aussi
    if let Some(sec_addr) = info.secondary_table_address {
        unsafe {
            let _ = patch_type0_at(sec_addr, 0x10000, config);
            let _ = patch_type1_at(sec_addr, 0x10000, config);
            let _ = patch_type2_at(sec_addr, 0x10000, config);
            let _ = patch_type3_at(sec_addr, 0x10000, config);
            let _ = patch_type4_at(sec_addr, 0x10000, config);
            let _ = patch_type11_at(sec_addr, 0x10000, config);
            let _ = patch_type17_at(sec_addr, 0x10000, config);
        }
    }

    // Synchroniser l'UUID dans la base interne du protocole PI/DXE si présent
    let wire_uuid = utils::uuid_to_smbios_bytes(&config.uuid);
    report.protocol_synced = smbios::update_uuid_via_protocol(&wire_uuid);

    // Recalculer les checksums des Entry Points (_SM_ / _DMI_ / _SM3_)
    info.recalculate_checksums();

    report
}

/// Lit toutes les valeurs SMBIOS originales **avant** application des patchs.
///
/// # Safety
/// `info` doit décrire une table SMBIOS valide en mémoire firmware.
pub unsafe fn read_originals_info(info: &SmbiosTableInfo) -> OriginalSmbiosData {
    let mut orig = OriginalSmbiosData::default();
    let addr = info.table_address;
    let max_len = info.table_max_length;

    // Type 0 : BIOS Vendor, Version, Release Date
    if let Some(table) = smbios::find_by_type_at(addr, max_len, smbios::SMBIOS_TYPE_BIOS, 0) {
        if let Some(type0) = table.as_type0() {
            let tbl_ptr = table.ptr.raw;
            let hdr_len = table.header_length();
            unsafe {
                smbios::read_smbios_string(tbl_ptr, hdr_len, type0.vendor, &mut orig.bios_vendor);
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type0.bios_version,
                    &mut orig.bios_version,
                );
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type0.bios_release_date,
                    &mut orig.bios_date,
                );
            }
        }
    }

    // Type 1 : UUID (converti du format mixed-endian en ordre d'affichage) + System Serial + SKU + Manufacturer
    if let Some(table) = smbios::find_by_type_at(addr, max_len, smbios::SMBIOS_TYPE_SYSTEM, 0) {
        if let Some(type1) = table.as_type1() {
            let raw_uuid = type1.uuid;
            orig.uuid = utils::uuid_to_smbios_bytes(&raw_uuid);
            let tbl_ptr = table.ptr.raw;
            let hdr_len = table.header_length();
            unsafe {
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type1.manufacturer,
                    &mut orig.system_manufacturer,
                );
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type1.product_name,
                    &mut orig.system_product,
                );
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type1.serial_number,
                    &mut orig.system_serial,
                );
                if hdr_len >= 0x1A {
                    smbios::read_smbios_string(
                        tbl_ptr,
                        hdr_len,
                        type1.sku_number,
                        &mut orig.system_sku,
                    );
                }
            }
        }
    }

    // Type 2 : Baseboard Manufacturer, Model, Serial, Asset Tag
    if let Some(table) = smbios::find_by_type_at(addr, max_len, smbios::SMBIOS_TYPE_BASEBOARD, 0) {
        if let Some(type2) = table.as_type2() {
            let tbl_ptr = table.ptr.raw;
            let hdr_len = table.header_length();
            unsafe {
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type2.manufacturer,
                    &mut orig.baseboard_manufacturer,
                );
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type2.product_name,
                    &mut orig.baseboard_model,
                );
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type2.serial_number,
                    &mut orig.baseboard_serial,
                );
                if hdr_len >= 0x09 {
                    smbios::read_smbios_string(
                        tbl_ptr,
                        hdr_len,
                        type2.asset_tag,
                        &mut orig.baseboard_asset_tag,
                    );
                }
            }
        }
    }

    // Type 3 : Chassis Serial + Asset Tag
    if let Some(table) = smbios::find_by_type_at(addr, max_len, smbios::SMBIOS_TYPE_ENCLOSURE, 0) {
        if let Some(type3) = table.as_type3() {
            let tbl_ptr = table.ptr.raw;
            let hdr_len = table.header_length();
            unsafe {
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type3.serial_number,
                    &mut orig.chassis_serial,
                );
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type3.asset_tag,
                    &mut orig.chassis_asset_tag,
                );
            }
        }
    }

    // Type 4 : Processor Serial
    if let Some(table) = smbios::find_by_type_at(addr, max_len, smbios::SMBIOS_TYPE_PROCESSOR, 0) {
        if let Some(type4) = table.as_type4() {
            let tbl_ptr = table.ptr.raw;
            let hdr_len = table.header_length();
            unsafe {
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type4.serial_number,
                    &mut orig.processor_serial,
                );
            }
        }
    }

    // Type 17 : Memory Devices (barrettes RAM peuplées)
    for slot_idx in 0..32 {
        let Some(table) =
            smbios::find_by_type_at(addr, max_len, smbios::SMBIOS_TYPE_MEMORY_DEVICE, slot_idx)
        else {
            break;
        };
        let Some(type17) = table.as_type17() else {
            continue;
        };
        let dimm_size = type17.size;
        if dimm_size == 0 || dimm_size == 0xFFFF {
            continue;
        }
        if orig.ram_dimm_count == 0 {
            let tbl_ptr = table.ptr.raw;
            let hdr_len = table.header_length();
            unsafe {
                smbios::read_smbios_string(
                    tbl_ptr,
                    hdr_len,
                    type17.serial_number,
                    &mut orig.first_ram_serial,
                );
                if hdr_len >= 0x1B {
                    smbios::read_smbios_string(
                        tbl_ptr,
                        hdr_len,
                        type17.part_number,
                        &mut orig.first_ram_part,
                    );
                }
            }
        }
        orig.ram_dimm_count += 1;
    }

    orig
}

// ─── Tests unitaires ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_originals_and_patch_all_info_on_mock_table() {
        let mut buf = [0u8; 256];
        let mut pos = 0usize;

        // Type 1 (System)
        buf[pos] = smbios::SMBIOS_TYPE_SYSTEM;
        buf[pos + 1] = 0x1B;
        buf[pos + 2] = 0x01;
        buf[pos + 3] = 0x00;
        buf[pos + 4] = 1; // manufacturer
        buf[pos + 5] = 2; // product_name
        buf[pos + 6] = 3; // version
        buf[pos + 7] = 4; // serial_number
        buf[pos + 25] = 5; // sku_number
        buf[pos + 26] = 6; // family
        pos += 0x1B;
        let s1 = b"ASUS\0ROG STRIX\01.0\0M80-12345678\0SKU-1234\0Family\0\0";
        buf[pos..pos + s1.len()].copy_from_slice(s1);
        pos += s1.len();

        // Type 17 (RAM DIMM)
        buf[pos] = smbios::SMBIOS_TYPE_MEMORY_DEVICE;
        buf[pos + 1] = 0x1B;
        buf[pos + 2] = 0x10;
        buf[pos + 3] = 0x00;
        buf[pos + 0x0C] = 0x00;
        buf[pos + 0x0D] = 0x40; // 16384 MB
        buf[pos + 0x17] = 1;
        buf[pos + 0x18] = 2; // serial_number
        buf[pos + 0x19] = 3; // asset_tag
        buf[pos + 0x1A] = 4; // part_number
        pos += 0x1B;
        let s17 = b"G.Skill\00123ABCD\0ASSET01\0F5-6000J3038F16G\0\0";
        buf[pos..pos + s17.len()].copy_from_slice(s17);
        pos += s17.len();

        // Type 127 (End)
        buf[pos] = smbios::SMBIOS_TYPE_END_OF_TABLE;
        buf[pos + 1] = 4;
        buf[pos + 2] = 0xFF;
        buf[pos + 3] = 0xFF;
        buf[pos + 4] = 0;
        buf[pos + 5] = 0;

        let info = SmbiosTableInfo {
            table_address: buf.as_mut_ptr() as usize,
            secondary_table_address: None,
            table_max_length: buf.len(),
            major_version: 3,
            minor_version: 6,
            is_64bit: true,
            entry_2x: None,
            entry_3x: None,
        };

        let orig = unsafe { read_originals_info(&info) };
        assert_eq!(orig.ram_dimm_count, 1);
        assert_eq!(utils::str_len(&orig.system_serial), 12);
        assert_eq!(utils::str_len(&orig.first_ram_serial), 8);

        let mut cfg = SpoofConfig::default();
        cfg.generate_from_originals(&orig);

        let report = unsafe { patch_all_info(&info, &cfg) };
        assert!(report.type1_patched);
        assert_eq!(report.type17_patched, 1);

        // Relire après patch : l'UUID et le serial RAM doivent avoir changé sans corrompre la table
        let after = unsafe { read_originals_info(&info) };
        assert_eq!(after.uuid, cfg.uuid);
        assert_ne!(after.first_ram_serial, orig.first_ram_serial);
        assert_eq!(after.ram_dimm_count, 1);
    }
}
