//! Patching des tables SMBIOS en mémoire.
//!
//! ## Ce que tu apprends ici :
//! - Manipulation mémoire brute avec `unsafe` pour modifier le firmware
//! - Pattern matching sur les types de structures SMBIOS
//! - Comment la philosophie Rust (safety) interagit avec le bare-metal
//! - Pourquoi chaque `unsafe` doit être documenté et justifié

use crate::smbios;
use crate::persistence::SpoofConfig;

/// Patche le Type 1 (System Information).
///
/// Modifie :
/// - UUID (16 bytes dans la structure)
/// - System Serial Number (chaîne dans la table de chaînes)
///
/// ## Safety
/// `entry` doit pointer vers un SMBIOS Entry Point valide.
/// On modifie la mémoire physique pointée par les tables SMBIOS.
pub unsafe fn patch_type1(entry: &smbios::SmbiosEntry, config: &SpoofConfig) -> bool {
    let table = smbios::find_by_type(entry, smbios::SMBIOS_TYPE_SYSTEM, 0);
    let table = match table {
        Some(t) => t,
        None => return false,
    };

    let type1 = match table.as_type1() {
        Some(t) => t,
        None => return false,
    };

    let table_ptr = table.ptr.raw as *mut u8;
    let header_len = table.header_length();

    // 1. Spoof UUID (directement dans la structure formatée)
    // Le champ UUID est à un offset fixe dans Type1.
    // On utilise pointer arithmetic pour y accéder.
    let uuid_offset = 8; // Après header (4) + 4 champs UINT8
    let uuid_ptr = unsafe { table_ptr.add(uuid_offset) as *mut [u8; 16] };
    unsafe {
        *uuid_ptr = config.uuid;
    }

    // 2. Spoof System Serial Number (dans la table de chaînes)
    // SAFETY: copie locale pour éviter les refs non-alignées sur packed struct
    let system_serial = config.system_serial;
    if type1.serial_number != 0 && system_serial[0] != 0 {
        // Convertir UTF-16 → ASCII pour la table de chaînes
        let mut ascii_buf = [0u8; 64];
        for (i, &ch) in system_serial.iter().enumerate() {
            if ch == 0 || i >= 63 {
                break;
            }
            ascii_buf[i] = if ch < 128 { ch as u8 } else { b'?' };
        }

        unsafe {
            smbios::edit_string(
                table_ptr,
                header_len,
                type1.serial_number,
                &ascii_buf,
            );
        }
    }

    true
}

/// Patche le Type 2 (Baseboard Information).
///
/// Modifie :
/// - Baseboard Serial Number
pub unsafe fn patch_type2(entry: &smbios::SmbiosEntry, config: &SpoofConfig) -> bool {
    let table = smbios::find_by_type(entry, smbios::SMBIOS_TYPE_BASEBOARD, 0);
    let table = match table {
        Some(t) => t,
        None => return false,
    };

    let type2 = match table.as_type2() {
        Some(t) => t,
        None => return false,
    };

    let table_ptr = table.ptr.raw as *mut u8;
    let header_len = table.header_length();

    // Spoof Baseboard Serial Number
    let baseboard_serial = config.baseboard_serial;
    if type2.serial_number != 0 && baseboard_serial[0] != 0 {
        let mut ascii_buf = [0u8; 64];
        for (i, &ch) in baseboard_serial.iter().enumerate() {
            if ch == 0 || i >= 63 {
                break;
            }
            ascii_buf[i] = if ch < 128 { ch as u8 } else { b'?' };
        }

        unsafe {
            smbios::edit_string(
                table_ptr,
                header_len,
                type2.serial_number,
                &ascii_buf,
            );
        }
    }

    true
}

/// Patche le Type 3 (System Enclosure / Chassis).
///
/// Modifie :
/// - Chassis Serial Number
///
/// ## Safety
/// `entry` doit pointer vers un SMBIOS Entry Point valide.
/// On modifie la mémoire physique pointée par les tables SMBIOS.
pub unsafe fn patch_type3(entry: &smbios::SmbiosEntry, config: &SpoofConfig) -> bool {
    let table = smbios::find_by_type(entry, smbios::SMBIOS_TYPE_ENCLOSURE, 0);
    let table = match table {
        Some(t) => t,
        None => return false,
    };

    let type3 = match table.as_type3() {
        Some(t) => t,
        None => return false,
    };

    let table_ptr = table.ptr.raw as *mut u8;
    let header_len = table.header_length();

    // Spoof Chassis Serial Number
    let chassis_serial = config.chassis_serial;
    if type3.serial_number != 0 && chassis_serial[0] != 0 {
        let mut ascii_buf = [0u8; 64];
        for (i, &ch) in chassis_serial.iter().enumerate() {
            if ch == 0 || i >= 63 {
                break;
            }
            ascii_buf[i] = if ch < 128 { ch as u8 } else { b'?' };
        }

        unsafe {
            smbios::edit_string(
                table_ptr,
                header_len,
                type3.serial_number,
                &ascii_buf,
            );
        }
    }

    true
}

/// Patche le Type 4 (Processor Information).
///
/// Modifie :
/// - Processor Serial Number
pub unsafe fn patch_type4(entry: &smbios::SmbiosEntry, config: &SpoofConfig) -> bool {
    let table = smbios::find_by_type(entry, smbios::SMBIOS_TYPE_PROCESSOR, 0);
    let table = match table {
        Some(t) => t,
        None => return false,
    };

    let type4 = match table.as_type4() {
        Some(t) => t,
        None => return false,
    };

    let table_ptr = table.ptr.raw as *mut u8;
    let header_len = table.header_length();

    // Spoof Processor Serial Number (si le champ existe)
    let processor_serial = config.processor_serial;
    if type4.serial_number != 0 && processor_serial[0] != 0 {
        let mut ascii_buf = [0u8; 64];
        for (i, &ch) in processor_serial.iter().enumerate() {
            if ch == 0 || i >= 63 {
                break;
            }
            ascii_buf[i] = if ch < 128 { ch as u8 } else { b'?' };
        }

        unsafe {
            smbios::edit_string(
                table_ptr,
                header_len,
                type4.serial_number,
                &ascii_buf,
            );
        }
    }

    true
}

/// Applique tous les patchs SMBIOS.
///
/// Modifie les Types 1 (System), 2 (Baseboard), 3 (Chassis) et 4 (Processor).
/// Les modifications sont faites DIRECTEMENT dans la mémoire physique
/// où le firmware a chargé les tables SMBIOS.
///
/// ## Safety
/// Cette fonction modifie la mémoire du firmware. Les modifications
/// sont volatiles (disparaissent au prochain reboot, d'où le besoin
/// de ré-exécuter le spoofer à chaque démarrage).
pub unsafe fn patch_all(entry: &smbios::SmbiosEntry, config: &SpoofConfig) {
    patch_type1(entry, config);
    patch_type2(entry, config);
    patch_type3(entry, config);
    patch_type4(entry, config);
}

/// Lit les valeurs SMBIOS originales avant spoof.
/// Utile pour l'affichage avant/après.
pub unsafe fn read_originals(
    entry: &smbios::SmbiosEntry,
    uuid: &mut [u8; 16],
    system_serial: &mut [u16; 64],
    baseboard_serial: &mut [u16; 64],
    baseboard_model: &mut [u16; 64],
    processor_serial: &mut [u16; 64],
    chassis_serial: &mut [u16; 64],
) {
    // Valeurs par défaut
    *uuid = [0u8; 16];
    system_serial[0] = 0;
    baseboard_serial[0] = 0;
    baseboard_model[0] = 0;
    processor_serial[0] = 0;
    chassis_serial[0] = 0;

    // Type 1 : UUID + System Serial
    if let Some(table) = smbios::find_by_type(entry, smbios::SMBIOS_TYPE_SYSTEM, 0) {
        if let Some(type1) = table.as_type1() {
            *uuid = type1.uuid;
            if type1.serial_number != 0 {
                let tbl_ptr = table.ptr.raw;
                let hdr_len = table.header_length();
                smbios::read_smbios_string(tbl_ptr, hdr_len, type1.serial_number, system_serial);
            }
        }
    }

    // Type 2 : Baseboard Serial + Model
    if let Some(table) = smbios::find_by_type(entry, smbios::SMBIOS_TYPE_BASEBOARD, 0) {
        if let Some(type2) = table.as_type2() {
            let tbl_ptr = table.ptr.raw;
            let hdr_len = table.header_length();
            if type2.serial_number != 0 {
                smbios::read_smbios_string(tbl_ptr, hdr_len, type2.serial_number, baseboard_serial);
            }
            if type2.product_name != 0 {
                smbios::read_smbios_string(tbl_ptr, hdr_len, type2.product_name, baseboard_model);
            }
        }
    }

    // Type 4 : Processor Serial
    if let Some(table) = smbios::find_by_type(entry, smbios::SMBIOS_TYPE_PROCESSOR, 0) {
        if let Some(type4) = table.as_type4() {
            if type4.serial_number != 0 {
                let tbl_ptr = table.ptr.raw;
                let hdr_len = table.header_length();
                smbios::read_smbios_string(tbl_ptr, hdr_len, type4.serial_number, processor_serial);
            }
        }
    }

    // Type 3 : Chassis Serial
    if let Some(table) = smbios::find_by_type(entry, smbios::SMBIOS_TYPE_ENCLOSURE, 0) {
        if let Some(type3) = table.as_type3() {
            if type3.serial_number != 0 {
                let tbl_ptr = table.ptr.raw;
                let hdr_len = table.header_length();
                smbios::read_smbios_string(tbl_ptr, hdr_len, type3.serial_number, chassis_serial);
            }
        }
    }
}
