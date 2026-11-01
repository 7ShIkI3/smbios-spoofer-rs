//! Structures et fonctions de navigation et d'édition SMBIOS (2.x 32-bit et 3.0 64-bit).
//!
//! Supporte :
//! - Entry Point 32-bit (`_SM_` / `_DMI_`, SMBIOS 2.x)
//! - Entry Point 64-bit (`_SM3_`, SMBIOS 3.0+)
//! - Recalcul des checksums d'Entry Point après patch
//! - Types 0 (BIOS), 1 (System), 2 (Baseboard), 3 (Chassis), 4 (Processor),
//!   11 (OEM Strings) et 17 (Memory Device / RAM DIMM)
//! - Édition en place sans corruption de la table de chaînes (préservation de `old_len`
//!   et du double null terminal `\0\0`)

// ─── Constantes SMBIOS ─────────────────────────────────────────────────────

pub const SMBIOS_TYPE_BIOS: u8 = 0;
pub const SMBIOS_TYPE_SYSTEM: u8 = 1;
pub const SMBIOS_TYPE_BASEBOARD: u8 = 2;
pub const SMBIOS_TYPE_ENCLOSURE: u8 = 3;
pub const SMBIOS_TYPE_PROCESSOR: u8 = 4;
pub const SMBIOS_TYPE_OEM_STRINGS: u8 = 11;
pub const SMBIOS_TYPE_MEMORY_DEVICE: u8 = 17;
pub const SMBIOS_TYPE_END_OF_TABLE: u8 = 0x7F;

// GUID de la table SMBIOS 2.x (32-bit) dans la table de configuration UEFI
pub const SMBIOS_TABLE_GUID: uefi::Guid = uefi::Guid::new(
    [0x31, 0x2D, 0x9D, 0xEB], // time_low: 0xEB9D2D31
    [0x88, 0x2D],             // time_mid: 0x2D88
    [0xD3, 0x11],             // time_high_and_version: 0x11D3
    0x9A,                     // clock_seq_high_and_reserved
    0x16,                     // clock_seq_low
    [0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D],
);

// GUID de la table SMBIOS 3.0+ (64-bit) dans la table de configuration UEFI
pub const SMBIOS3_TABLE_GUID: uefi::Guid = uefi::Guid::new(
    [0x44, 0x15, 0xFD, 0xF2], // time_low: 0xF2FD1544
    [0x94, 0x97],             // time_mid: 0x9794
    [0x2C, 0x4A],             // time_high_and_version: 0x4A2C
    0x99,                     // clock_seq_high_and_reserved
    0x2E,                     // clock_seq_low
    [0xE5, 0xBB, 0xCF, 0x20, 0xE3, 0x94],
);

/// GUID du protocole `EFI_SMBIOS_PROTOCOL` (PI Spec 1.6+).
pub const SMBIOS_PROTOCOL_GUID: uefi::Guid = uefi::Guid::new(
    [0xF6, 0x83, 0x35, 0x03], // time_low: 0x03583FF6
    [0x36, 0xCB],             // time_mid: 0xCB36
    [0x40, 0x49],             // time_high_and_version: 0x4940
    0x94,                     // clock_seq_high_and_reserved
    0x7E,                     // clock_seq_low
    [0xB9, 0xB3, 0x9F, 0x4A, 0xFA, 0xF7],
);

/// Vérifie si le protocole `EFI_SMBIOS_PROTOCOL` est disponible sur ce firmware.
#[cfg(target_os = "uefi")]
pub fn smbios_protocol_available() -> bool {
    uefi::boot::locate_handle_buffer(uefi::boot::SearchType::ByProtocol(&SMBIOS_PROTOCOL_GUID))
        .is_ok()
}

#[cfg(not(target_os = "uefi"))]
pub fn smbios_protocol_available() -> bool {
    false
}

// ─── Protocole EFI_SMBIOS_PROTOCOL (FFI PI Spec) ────────────────────────────

/// Représentation FFI de `EFI_SMBIOS_PROTOCOL` (PI Spec Vol. 5).
#[repr(C)]
#[cfg_attr(target_os = "uefi", uefi::proto::unsafe_protocol(SMBIOS_PROTOCOL_GUID))]
pub struct EfiSmbiosProtocol {
    pub add: unsafe extern "efiapi" fn(
        this: *const EfiSmbiosProtocol,
        producer_handle: *mut core::ffi::c_void,
        smbios_handle: *mut u16,
        record: *const SmbiosHeader,
    ) -> uefi::Status,
    pub update_string: unsafe extern "efiapi" fn(
        this: *const EfiSmbiosProtocol,
        smbios_handle: *mut u16,
        string_number: *mut usize,
        string: *const u8,
    ) -> uefi::Status,
    pub remove: unsafe extern "efiapi" fn(
        this: *const EfiSmbiosProtocol,
        smbios_handle: u16,
    ) -> uefi::Status,
    pub get_next: unsafe extern "efiapi" fn(
        this: *const EfiSmbiosProtocol,
        smbios_handle: *mut u16,
        record_type: *const u8,
        record: *mut *mut SmbiosHeader,
        producer_handle: *mut *mut core::ffi::c_void,
    ) -> uefi::Status,
    pub major_version: u8,
    pub minor_version: u8,
}

/// Met à jour une chaîne SMBIOS via `EFI_SMBIOS_PROTOCOL.UpdateString` si disponible.
/// Utile sur les firmwares qui synchronisent leur base DXE interne avec la table mémoire.
#[cfg(target_os = "uefi")]
pub fn update_string_via_protocol(handle: u16, field_index: u8, new_value_ascii: &[u8]) -> bool {
    if field_index == 0 || new_value_ascii.is_empty() {
        return false;
    }

    // Construire une chaîne C terminée par '\0' sans octets nuls internes
    let mut cstr = [0u8; 65];
    let actual_len = new_value_ascii
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(new_value_ascii.len())
        .min(64);
    if actual_len == 0 {
        return false;
    }
    cstr[..actual_len].copy_from_slice(&new_value_ascii[..actual_len]);
    cstr[actual_len] = 0;

    let Ok(proto_handle) = uefi::boot::get_handle_for_protocol::<EfiSmbiosProtocol>() else {
        return false;
    };
    let Ok(proto) = uefi::boot::open_protocol_exclusive::<EfiSmbiosProtocol>(proto_handle) else {
        return false;
    };

    let mut smbios_handle = handle;
    let mut string_num = field_index as usize;
    let proto_ptr: *const EfiSmbiosProtocol = &*proto;
    // SAFETY: `proto_ptr` est valide, `cstr` est terminé par un octet nul
    let status = unsafe {
        ((*proto_ptr).update_string)(
            proto_ptr,
            &mut smbios_handle,
            &mut string_num,
            cstr.as_ptr(),
        )
    };
    status == uefi::Status::SUCCESS
}

#[cfg(not(target_os = "uefi"))]
pub fn update_string_via_protocol(_handle: u16, _field_index: u8, _new_value_ascii: &[u8]) -> bool {
    false
}

/// Met à jour l'UUID du Type 1 dans la base interne `EFI_SMBIOS_PROTOCOL` via `GetNext`.
#[cfg(target_os = "uefi")]
pub fn update_uuid_via_protocol(uuid_mixed_endian: &[u8; 16]) -> bool {
    let Ok(proto_handle) = uefi::boot::get_handle_for_protocol::<EfiSmbiosProtocol>() else {
        return false;
    };
    let Ok(proto) = uefi::boot::open_protocol_exclusive::<EfiSmbiosProtocol>(proto_handle) else {
        return false;
    };

    let proto_ptr: *const EfiSmbiosProtocol = &*proto;
    let mut handle: u16 = 0xFFFE; // EFI_SMBIOS_HANDLE_PI_RESERVED (début d'itération)
    let target_type: u8 = SMBIOS_TYPE_SYSTEM;
    let mut record_ptr: *mut SmbiosHeader = core::ptr::null_mut();

    // SAFETY: appel FFI standard PI Spec GetNext pour chercher le record Type 1
    let status = unsafe {
        ((*proto_ptr).get_next)(
            proto_ptr,
            &mut handle,
            &target_type,
            &mut record_ptr,
            core::ptr::null_mut(),
        )
    };

    if status == uefi::Status::SUCCESS && !record_ptr.is_null() {
        // SAFETY: `record_ptr` pointe vers un enregistrement SMBIOS Type 1
        unsafe {
            let len = (*record_ptr).length as usize;
            if len >= core::mem::size_of::<SmbiosType1>() {
                let type1_mut = record_ptr.cast::<SmbiosType1>();
                (*type1_mut).uuid = *uuid_mixed_endian;
                return true;
            }
        }
    }
    false
}

#[cfg(not(target_os = "uefi"))]
pub fn update_uuid_via_protocol(_uuid_mixed_endian: &[u8; 16]) -> bool {
    false
}

// ─── Structs SMBIOS Entry Points (mapping mémoire exact) ───────────────────

/// Point d'entrée SMBIOS 2.x (Entry Point Structure 32-bit, `_SM_`).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SmbiosEntry {
    pub anchor_string: [u8; 4],       // "_SM_"
    pub checksum: u8,
    pub length: u8,
    pub major_version: u8,
    pub minor_version: u8,
    pub max_structure_size: u16,
    pub entry_point_revision: u8,
    pub formatted_area: [u8; 5],
    pub intermediate_anchor: [u8; 5], // "_DMI_"
    pub intermediate_checksum: u8,
    pub structure_table_length: u16,
    pub structure_table_address: u32, // Adresse physique 32-bit de la table
    pub number_of_structures: u16,
    pub bcd_revision: u8,
}

/// Point d'entrée SMBIOS 3.0+ (Entry Point Structure 64-bit, `_SM3_`).
/// Utilisé par la majorité des cartes mères UEFI modernes.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Smbios3Entry {
    pub anchor_string: [u8; 5],        // "_SM3_"
    pub checksum: u8,                  // Checksum de l'entry point (somme = 0)
    pub length: u8,                    // Longueur de l'entry point (24 octets = 0x18)
    pub major_version: u8,
    pub minor_version: u8,
    pub docrev: u8,
    pub entry_point_revision: u8,
    pub reserved: u8,
    pub structure_table_max_size: u32, // Taille maximale de la table
    pub structure_table_address: u64,  // Adresse physique 64-bit de la table
}

/// Vue unifiée sur l'Entry Point SMBIOS (2.x 32-bit et/ou 3.0 64-bit).
/// Certains firmwares exposent les deux tables (souvent pointant vers la même adresse
/// ou deux copies distinctes) : on conserve les deux pointeurs pour tout patcher.
#[derive(Debug, Clone, Copy)]
pub struct SmbiosTableInfo {
    pub table_address: usize,
    pub secondary_table_address: Option<usize>,
    pub table_max_length: usize,
    pub major_version: u8,
    pub minor_version: u8,
    pub is_64bit: bool,
    pub entry_2x: Option<*mut SmbiosEntry>,
    pub entry_3x: Option<*mut Smbios3Entry>,
}

impl SmbiosTableInfo {
    /// Recalcule les checksums des Entry Points (`_SM_` / `_DMI_` et `_SM3_`)
    /// pour garantir une table SMBIOS 100% conforme après modification.
    pub fn recalculate_checksums(&self) {
        if let Some(ptr_2x) = self.entry_2x {
            if !ptr_2x.is_null() {
                // SAFETY: `ptr_2x` pointe vers l'Entry Point 2.x en mémoire firmware
                unsafe {
                    recalculate_entry_2x_checksum(ptr_2x);
                }
            }
        }
        if let Some(ptr_3x) = self.entry_3x {
            if !ptr_3x.is_null() {
                // SAFETY: `ptr_3x` pointe vers l'Entry Point 3.0 en mémoire firmware
                unsafe {
                    recalculate_entry_3x_checksum(ptr_3x);
                }
            }
        }
    }
}

/// Recalcule l'intermediate checksum (`_DMI_`, octets `0x10..0x1F`) et l'entry point
/// checksum (`_SM_`, octets `0x00..length`) d'un `SmbiosEntry` 2.x.
///
/// # Safety
/// `entry_ptr` doit pointer vers un `SmbiosEntry` valide et accessible en écriture.
pub unsafe fn recalculate_entry_2x_checksum(entry_ptr: *mut SmbiosEntry) {
    let raw = entry_ptr.cast::<u8>();
    let len = unsafe { (*entry_ptr).length as usize };
    if !(0x1F..=64).contains(&len) {
        return;
    }

    // 1. Intermediate checksum (octets 0x10..0x1F, checksum à l'offset 0x15)
    unsafe {
        (*entry_ptr).intermediate_checksum = 0;
        let mut dmi_sum: u8 = 0;
        for i in 0x10..0x1F {
            dmi_sum = dmi_sum.wrapping_add(*raw.add(i));
        }
        (*entry_ptr).intermediate_checksum = 0u8.wrapping_sub(dmi_sum);

        // 2. Entry point global checksum (octets 0x00..len, checksum à l'offset 0x04)
        (*entry_ptr).checksum = 0;
        let mut ep_sum: u8 = 0;
        for i in 0..len {
            ep_sum = ep_sum.wrapping_add(*raw.add(i));
        }
        (*entry_ptr).checksum = 0u8.wrapping_sub(ep_sum);
    }
}

/// Recalcule le checksum d'un `Smbios3Entry` 3.0 (`_SM3_`, octets `0x00..length`).
///
/// # Safety
/// `entry_ptr` doit pointer vers un `Smbios3Entry` valide et accessible en écriture.
pub unsafe fn recalculate_entry_3x_checksum(entry_ptr: *mut Smbios3Entry) {
    let raw = entry_ptr.cast::<u8>();
    let len = unsafe { (*entry_ptr).length as usize };
    if !(24..=64).contains(&len) {
        return;
    }

    unsafe {
        (*entry_ptr).checksum = 0;
        let mut sum: u8 = 0;
        for i in 0..len {
            sum = sum.wrapping_add(*raw.add(i));
        }
        (*entry_ptr).checksum = 0u8.wrapping_sub(sum);
    }
}

// ─── En-tête et Structures SMBIOS spécifiques ──────────────────────────────

/// En-tête commun à toutes les structures SMBIOS (4 octets).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SmbiosHeader {
    pub type_: u8,   // Type de structure (0=BIOS, 1=System, etc.)
    pub length: u8,  // Taille de la partie formatée
    pub handle: u16, // Identifiant unique
}

/// Type 0 : BIOS Information
#[repr(C, packed)]
pub struct SmbiosType0 {
    pub header: SmbiosHeader,
    pub vendor: u8,            // Index de chaîne (0x04)
    pub bios_version: u8,      // Index de chaîne (0x05)
    pub bios_start_segment: u16,
    pub bios_release_date: u8, // Index de chaîne (0x08)
    pub bios_rom_size: u8,
}

/// Type 1 : System Information
#[repr(C, packed)]
pub struct SmbiosType1 {
    pub header: SmbiosHeader,
    pub manufacturer: u8,  // Index chaîne (0x04)
    pub product_name: u8,  // Index chaîne (0x05)
    pub version: u8,       // Index chaîne (0x06)
    pub serial_number: u8, // Index chaîne (0x07) ← SPOOF
    pub uuid: [u8; 16],    // UUID système (0x08..0x17) ← SPOOF (mixed-endian)
    pub wake_up_type: u8,  // (0x18)
    pub sku_number: u8,    // Index chaîne (0x19) ← SPOOF
    pub family: u8,        // Index chaîne (0x1A)
}

/// Type 2 : Baseboard (Motherboard) Information
#[repr(C, packed)]
pub struct SmbiosType2 {
    pub header: SmbiosHeader,
    pub manufacturer: u8,        // Index chaîne (0x04)
    pub product_name: u8,        // Index chaîne (0x05)
    pub version: u8,             // Index chaîne (0x06)
    pub serial_number: u8,       // Index chaîne (0x07) ← SPOOF
    pub asset_tag: u8,           // Index chaîne (0x08) ← SPOOF
    pub feature_flags: u8,       // (0x09)
    pub location_in_chassis: u8, // Index chaîne (0x0A)
    pub chassis_handle: u16,     // (0x0B..0x0C)
    pub board_type: u8,          // (0x0D)
}

/// Type 3 : System Enclosure / Chassis
#[repr(C, packed)]
pub struct SmbiosType3 {
    pub header: SmbiosHeader,
    pub manufacturer: u8,       // Index chaîne (0x04)
    pub enclosure_type: u8,     // (0x05)
    pub version: u8,            // Index chaîne (0x06)
    pub serial_number: u8,      // Index chaîne (0x07) ← SPOOF
    pub asset_tag: u8,          // Index chaîne (0x08) ← SPOOF
    pub boot_up_state: u8,      // (0x09)
    pub power_supply_state: u8, // (0x0A)
    pub thermal_state: u8,      // (0x0B)
    pub security_status: u8,    // (0x0C)
    pub oem_defined: u32,       // (0x0D..0x10)
    pub height: u8,             // (0x11)
    pub number_power_cords: u8, // (0x12)
    pub contained_element_count: u8,         // (0x13)
    pub contained_element_record_length: u8, // (0x14)
}

/// Type 4 : Processor Information
#[repr(C, packed)]
pub struct SmbiosType4 {
    pub header: SmbiosHeader,
    pub socket_designation: u8,     // Index chaîne (0x04)
    pub processor_type: u8,         // (0x05)
    pub processor_family: u8,       // (0x06)
    pub processor_manufacturer: u8, // Index chaîne (0x07)
    pub processor_id: u64,          // CPUID (0x08..0x0F)
    pub processor_version: u8,      // Index chaîne (0x10)
    pub voltage: u8,                // (0x11)
    pub external_clock: u16,        // (0x12..0x13)
    pub max_speed: u16,             // (0x14..0x15)
    pub current_speed: u16,         // (0x16..0x17)
    pub status: u8,                 // (0x18)
    pub processor_upgrade: u8,      // (0x19)
    pub l1_cache_handle: u16,       // (0x1A..0x1B)
    pub l2_cache_handle: u16,       // (0x1C..0x1D)
    pub l3_cache_handle: u16,       // (0x1E..0x1F)
    pub serial_number: u8,          // Index chaîne (0x20) ← SPOOF
    pub asset_tag: u8,              // Index chaîne (0x21) ← SPOOF
    pub part_number: u8,            // Index chaîne (0x22) ← SPOOF
}

/// Type 11 : OEM Strings
#[repr(C, packed)]
pub struct SmbiosType11 {
    pub header: SmbiosHeader,
    pub count: u8, // Nombre d'OEM strings (0x04)
}

/// Type 17 : Memory Device (barrettes RAM DIMM)
/// Les anti-cheats modernes lisent systématiquement le `serial_number` (0x18)
/// et `part_number` (0x1A) de chaque barrette RAM installée.
#[repr(C, packed)]
pub struct SmbiosType17 {
    pub header: SmbiosHeader,                   // 0x00..0x03
    pub physical_memory_array_handle: u16,      // 0x04..0x05
    pub memory_error_information_handle: u16,   // 0x06..0x07
    pub total_width: u16,                       // 0x08..0x09
    pub data_width: u16,                        // 0x0A..0x0B
    pub size: u16,                              // 0x0C..0x0D (0 = slot vide)
    pub form_factor: u8,                        // 0x0E
    pub device_set: u8,                         // 0x0F
    pub device_locator: u8,                     // 0x10 (Index chaîne)
    pub bank_locator: u8,                       // 0x11 (Index chaîne)
    pub memory_type: u8,                        // 0x12
    pub type_detail: u16,                       // 0x13..0x14
    pub speed: u16,                             // 0x15..0x16
    pub manufacturer: u8,                       // 0x17 (Index chaîne)
    pub serial_number: u8,                      // 0x18 (Index chaîne ← SPOOF)
    pub asset_tag: u8,                          // 0x19 (Index chaîne ← SPOOF)
    pub part_number: u8,                        // 0x1A (Index chaîne)
}

// ─── Union pour naviguer les structures ────────────────────────────────────

/// Union permettant d'accéder à une structure SMBIOS sous son type concret.
#[repr(C)]
pub union SmbiosStructPtr {
    pub raw: *const u8,
    pub header: *const SmbiosHeader,
    pub type0: *const SmbiosType0,
    pub type1: *const SmbiosType1,
    pub type2: *const SmbiosType2,
    pub type3: *const SmbiosType3,
    pub type4: *const SmbiosType4,
    pub type11: *const SmbiosType11,
    pub type17: *const SmbiosType17,
}

/// Wrapper typé autour d'un pointeur de structure SMBIOS.
pub struct SmbiosStruct {
    pub(crate) ptr: SmbiosStructPtr,
    pub type_: u8,
}

impl SmbiosStruct {
    /// Crée depuis un pointeur brut.
    ///
    /// # Safety
    /// L'appelant doit s'assurer que `ptr` pointe vers une structure SMBIOS valide.
    pub unsafe fn from_raw(ptr: *const u8) -> Option<Self> {
        if ptr.is_null() {
            return None;
        }
        let type_ = unsafe { (*ptr.cast::<SmbiosHeader>()).type_ };
        Some(Self {
            ptr: SmbiosStructPtr { raw: ptr },
            type_,
        })
    }

    /// Longueur de la partie formatée (depuis le header).
    pub fn header_length(&self) -> usize {
        // SAFETY: `self.ptr.header` est valide tant que `SmbiosStruct` est valide
        unsafe { (*self.ptr.header).length as usize }
    }

    /// Identifiant unique (handle) de la structure SMBIOS.
    pub fn handle(&self) -> u16 {
        // SAFETY: copie par valeur du champ `u16` d'une struct `packed`
        unsafe { (*self.ptr.header).handle }
    }

    /// Accède au header.
    pub fn header(&self) -> &SmbiosHeader {
        unsafe { &*self.ptr.header }
    }

    /// Accède au Type 0 (BIOS Information).
    pub fn as_type0(&self) -> Option<&SmbiosType0> {
        if self.type_ == SMBIOS_TYPE_BIOS && self.header_length() >= 0x09 {
            unsafe { Some(&*self.ptr.type0) }
        } else {
            None
        }
    }

    /// Accède au Type 1 (System).
    pub fn as_type1(&self) -> Option<&SmbiosType1> {
        if self.type_ == SMBIOS_TYPE_SYSTEM && self.header_length() >= 0x08 {
            unsafe { Some(&*self.ptr.type1) }
        } else {
            None
        }
    }

    /// Accède au Type 2 (Baseboard).
    pub fn as_type2(&self) -> Option<&SmbiosType2> {
        if self.type_ == SMBIOS_TYPE_BASEBOARD && self.header_length() >= 0x08 {
            unsafe { Some(&*self.ptr.type2) }
        } else {
            None
        }
    }

    /// Accède au Type 3 (Chassis).
    pub fn as_type3(&self) -> Option<&SmbiosType3> {
        if self.type_ == SMBIOS_TYPE_ENCLOSURE && self.header_length() >= 0x09 {
            unsafe { Some(&*self.ptr.type3) }
        } else {
            None
        }
    }

    /// Accède au Type 4 (Processor).
    pub fn as_type4(&self) -> Option<&SmbiosType4> {
        if self.type_ == SMBIOS_TYPE_PROCESSOR && self.header_length() >= 0x21 {
            unsafe { Some(&*self.ptr.type4) }
        } else {
            None
        }
    }

    /// Accède au Type 11 (OEM Strings).
    pub fn as_type11(&self) -> Option<&SmbiosType11> {
        if self.type_ == SMBIOS_TYPE_OEM_STRINGS && self.header_length() >= 0x05 {
            unsafe { Some(&*self.ptr.type11) }
        } else {
            None
        }
    }

    /// Accède au Type 17 (Memory Device / RAM).
    pub fn as_type17(&self) -> Option<&SmbiosType17> {
        if self.type_ == SMBIOS_TYPE_MEMORY_DEVICE && self.header_length() >= 0x19 {
            unsafe { Some(&*self.ptr.type17) }
        } else {
            None
        }
    }

    /// Pointeur vers la table de chaînes (juste après la partie formatée).
    pub fn string_table_ptr(&self) -> *const u8 {
        unsafe { self.ptr.raw.add(self.header_length()) }
    }

    /// Pointeur mutable vers la table de chaînes (pour édition).
    pub fn string_table_ptr_mut(&mut self) -> *mut u8 {
        unsafe { (self.ptr.raw as *mut u8).add(self.header_length()) }
    }
}

// ─── Navigation des tables SMBIOS ──────────────────────────────────────────

/// Calcule la longueur totale d'une structure SMBIOS (partie formatée + chaînes + `\0\0`).
///
/// # Safety
/// `ptr` doit pointer vers le début d'une structure SMBIOS valide.
pub unsafe fn table_length(ptr: *const u8) -> usize {
    let header = unsafe { &*ptr.cast::<SmbiosHeader>() };
    let header_len = (header.length as usize).max(core::mem::size_of::<SmbiosHeader>());
    let string_start = unsafe { ptr.add(header_len) };

    // Parcourir la table de chaînes jusqu'au double null (\0\0), avec garde-fou anti-boucle infinie
    let mut end = string_start;
    let max_scan = 4096usize;
    for _ in 0..max_scan {
        if unsafe { *end == 0 && *end.add(1) == 0 } {
            break;
        }
        end = unsafe { end.add(1) };
    }

    // +2 pour inclure le double null terminal
    unsafe { end.offset_from(ptr) as usize + 2 }
}

/// Trouve la `index`-ième structure SMBIOS de type `type_` à partir d'une adresse de table
/// et d'une taille maximale.
pub fn find_by_type_at(
    table_address: usize,
    table_max_length: usize,
    type_: u8,
    index: usize,
) -> Option<SmbiosStruct> {
    if table_address == 0 {
        return None;
    }

    let start = table_address as *const u8;
    let max_len = if table_max_length == 0 {
        0x10000
    } else {
        table_max_length
    };
    let mut offset = 0usize;
    let mut type_index = 0usize;

    while offset + core::mem::size_of::<SmbiosHeader>() <= max_len {
        let current = unsafe { start.add(offset) };
        let header = unsafe { &*current.cast::<SmbiosHeader>() };

        // Structure invalide (longueur < 4 octets)
        if (header.length as usize) < core::mem::size_of::<SmbiosHeader>() {
            return None;
        }

        // Fin de table SMBIOS (Type 127 / 0x7F)
        if header.type_ == SMBIOS_TYPE_END_OF_TABLE {
            if type_ == SMBIOS_TYPE_END_OF_TABLE && type_index == index {
                return unsafe { SmbiosStruct::from_raw(current) };
            }
            return None;
        }

        if header.type_ == type_ {
            if type_index == index {
                return unsafe { SmbiosStruct::from_raw(current) };
            }
            type_index += 1;
        }

        let len = unsafe { table_length(current) };
        if len == 0 || offset + len > max_len {
            return None;
        }
        offset += len;
    }

    None
}

/// Trouve une structure SMBIOS par type via `SmbiosTableInfo` (2.x ou 3.0).
pub fn find_by_type_info(info: &SmbiosTableInfo, type_: u8, index: usize) -> Option<SmbiosStruct> {
    find_by_type_at(info.table_address, info.table_max_length, type_, index)
}

/// Trouve une structure SMBIOS par type via `SmbiosEntry` 2.x (compatibilité).
pub fn find_by_type(entry: &SmbiosEntry, type_: u8, index: usize) -> Option<SmbiosStruct> {
    let addr = entry.structure_table_address as usize;
    let max_len = entry.structure_table_length as usize;
    find_by_type_at(addr, max_len, type_, index)
}

/// Compte le nombre d'occurrences d'un type SMBIOS donné dans la table.
pub fn count_by_type_info(info: &SmbiosTableInfo, type_: u8) -> usize {
    let mut count = 0usize;
    while find_by_type_info(info, type_, count).is_some() {
        count += 1;
        if count >= 128 {
            break;
        }
    }
    count
}

// ─── Édition des chaînes SMBIOS ────────────────────────────────────────────

/// Remplace une chaîne en place dans la table SMBIOS sans jamais corrompre les
/// terminateurs `\0` ni le double null final `\0\0`.
///
/// - Calcule `actual_len` en s'arrêtant au premier octet nul `0x00` de `new_value`
///   (évite d'écrire des octets nuls au milieu de la table si l'appelant passe un buffer fixe).
/// - Si `actual_len < old_len`, complète avec des espaces ASCII (`0x20`) pour conserver
///   exactement `old_len` octets non-nuls et ne jamais créer de double null prématuré.
///
/// # Safety
/// `table_ptr` doit pointer vers le début d'une structure SMBIOS valide et accessible en écriture.
pub unsafe fn edit_string(
    table_ptr: *mut u8,
    header_len: usize,
    field_index: u8,
    new_value: &[u8],
) {
    if table_ptr.is_null() || field_index == 0 {
        return;
    }

    // Longueur effective de la nouvelle chaîne (sans les octets 0x00 de fin de buffer)
    let actual_len = new_value
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(new_value.len());
    if actual_len == 0 {
        return;
    }

    // Si la table de chaînes est vide (commence immédiatement par \0\0), aucune chaîne n'existe
    let mut astr = unsafe { table_ptr.add(header_len) };
    if unsafe { *astr == 0 && *astr.add(1) == 0 } {
        return;
    }

    // Naviguer jusqu'à la `field_index`-ième chaîne (1-indexed)
    let mut index: u8 = 1;
    while index < field_index {
        // Avancer jusqu'au '\0' de fin de la chaîne courante
        while unsafe { *astr != 0 } {
            astr = unsafe { astr.add(1) };
        }
        // Passer le '\0'
        astr = unsafe { astr.add(1) };

        // Si l'octet suivant est aussi '\0', on a atteint le double null (fin de structure)
        if unsafe { *astr == 0 } {
            return;
        }
        index += 1;
    }

    // Calculer la longueur de la chaîne existante
    let old_len = {
        let mut len = 0usize;
        while unsafe { *astr.add(len) != 0 } {
            len += 1;
        }
        len
    };

    if old_len == 0 {
        return;
    }

    // Copier les octets non-nuls (tronqués à `old_len` si plus longs)
    let copy_len = actual_len.min(old_len);
    for (i, &val) in new_value.iter().enumerate().take(copy_len) {
        // Remplacer tout caractère de contrôle éventuel par un caractère imprimable
        let byte = if !(0x20..=0x7E).contains(&val) {
            b'X'
        } else {
            val
        };
        unsafe {
            *astr.add(i) = byte;
        }
    }

    // Padder avec des espaces si la nouvelle chaîne est plus courte que l'originale
    for i in copy_len..old_len {
        unsafe {
            *astr.add(i) = b' ';
        }
    }
}

// ─── Lecture des chaînes SMBIOS ────────────────────────────────────────────

/// Lit une chaîne depuis une structure SMBIOS et la convertit en UTF-16 null-terminated.
///
/// # Safety
/// `table_ptr` doit pointer vers une structure SMBIOS valide.
pub unsafe fn read_smbios_string(
    table_ptr: *const u8,
    header_len: usize,
    field_index: u8,
    output: &mut [u16],
) {
    if output.is_empty() {
        return;
    }
    output[0] = 0;

    if table_ptr.is_null() || field_index == 0 {
        return;
    }

    let mut astr = unsafe { table_ptr.add(header_len) };
    if unsafe { *astr == 0 && *astr.add(1) == 0 } {
        return;
    }

    let mut index: u8 = 1;
    while index < field_index {
        while unsafe { *astr != 0 } {
            astr = unsafe { astr.add(1) };
        }
        astr = unsafe { astr.add(1) };

        if unsafe { *astr == 0 } {
            return;
        }
        index += 1;
    }

    let max = output.len() - 1;
    let mut written = 0usize;
    for (i, out_slot) in output.iter_mut().enumerate().take(max) {
        let ch = unsafe { *astr.add(i) };
        if ch == 0 {
            break;
        }
        *out_slot = ch as u16;
        written = i + 1;
    }
    output[written] = 0;
}

// ─── Tests unitaires ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Construit une fausse table SMBIOS en mémoire avec Type 1 + Type 17 + Type 127 (End)
    fn build_mock_smbios_table() -> [u8; 256] {
        let mut buf = [0u8; 256];
        let mut pos = 0usize;

        // 1. Structure Type 1 (System Info), length = 0x1B (27 octets)
        buf[pos] = SMBIOS_TYPE_SYSTEM;
        buf[pos + 1] = 0x1B;
        buf[pos + 2] = 0x01; // handle low
        buf[pos + 3] = 0x00; // handle high
        buf[pos + 4] = 1; // manufacturer = string 1
        buf[pos + 5] = 2; // product_name = string 2
        buf[pos + 6] = 3; // version = string 3
        buf[pos + 7] = 4; // serial_number = string 4
        buf[pos + 25] = 5; // sku_number = string 5
        buf[pos + 26] = 6; // family = string 6
        pos += 0x1B;

        // Table de chaînes du Type 1 :
        // 1: "ASUS\0"
        // 2: "ROG STRIX\0"
        // 3: "1.0\0"
        // 4: "ABC123456789\0" (12 chars)
        // 5: "SKU-001\0"
        // 6: "Default\0"
        // Fin: "\0"
        let strings_t1 = b"ASUS\0ROG STRIX\01.0\0ABC123456789\0SKU-001\0Default\0\0";
        buf[pos..pos + strings_t1.len()].copy_from_slice(strings_t1);
        pos += strings_t1.len();

        // 2. Structure Type 17 (Memory Device), length = 0x1B (27 octets)
        buf[pos] = SMBIOS_TYPE_MEMORY_DEVICE;
        buf[pos + 1] = 0x1B;
        buf[pos + 2] = 0x10;
        buf[pos + 3] = 0x00;
        // size à l'offset 0x0C = 16384 MB (0x4000)
        buf[pos + 0x0C] = 0x00;
        buf[pos + 0x0D] = 0x40;
        buf[pos + 0x17] = 1; // manufacturer = string 1
        buf[pos + 0x18] = 2; // serial_number = string 2
        buf[pos + 0x19] = 3; // asset_tag = string 3
        buf[pos + 0x1A] = 4; // part_number = string 4
        pos += 0x1B;

        let strings_t17 = b"Corsair\01A2B3C4D\0AT-01\0CMK32GX5M2B5600C36\0\0";
        buf[pos..pos + strings_t17.len()].copy_from_slice(strings_t17);
        pos += strings_t17.len();

        // 3. Structure Type 127 (End-of-Table), length = 4, suivie de \0\0
        buf[pos] = SMBIOS_TYPE_END_OF_TABLE;
        buf[pos + 1] = 4;
        buf[pos + 2] = 0xFF;
        buf[pos + 3] = 0xFF;
        buf[pos + 4] = 0;
        buf[pos + 5] = 0;

        buf
    }

    #[test]
    fn test_edit_string_never_corrupts_table_with_fixed_buffer() {
        let mut table = build_mock_smbios_table();
        let addr = table.as_mut_ptr() as usize;

        // Vérifier qu'on trouve Type 1 et Type 17 avant édition
        let s1 = find_by_type_at(addr, table.len(), SMBIOS_TYPE_SYSTEM, 0).unwrap();
        let s1_len_before = unsafe { table_length(s1.ptr.raw) };

        // Simuler un buffer fixe de 64 octets rempli de 0 après 6 caractères ("XY9876\0\0...")
        // C'était le bug critique #1 : edit_string écrivait des 0x00 dans la table !
        let mut fixed_buf = [0u8; 64];
        fixed_buf[..6].copy_from_slice(b"XY9876");

        unsafe {
            edit_string(s1.ptr.raw as *mut u8, s1.header_length(), 4, &fixed_buf);
        }

        // La longueur totale de la structure Type 1 DOIT rester strictement identique !
        let s1_len_after = unsafe { table_length(s1.ptr.raw) };
        assert_eq!(s1_len_before, s1_len_after);

        // Les chaînes suivantes (5="SKU-001" et 6="Default") et la structure suivante (Type 17)
        // doivent toujours être intactes et accessibles !
        let mut sku = [0u16; 32];
        unsafe {
            read_smbios_string(s1.ptr.raw, s1.header_length(), 5, &mut sku);
        }
        assert_eq!(sku[0], b'S' as u16);
        assert_eq!(sku[1], b'K' as u16);
        assert_eq!(sku[2], b'U' as u16);

        // Et le Type 17 suivant doit toujours être trouvé !
        let s17 = find_by_type_at(addr, table.len(), SMBIOS_TYPE_MEMORY_DEVICE, 0);
        assert!(s17.is_some());
        let s17 = s17.unwrap();
        let t17 = s17.as_type17().unwrap();
        let size = t17.size;
        assert_eq!(size, 0x4000);
        assert_eq!(t17.serial_number, 2);
    }

    #[test]
    fn test_entry_checksum_recalculation_2x_and_3x() {
        let mut entry2 = SmbiosEntry {
            anchor_string: *b"_SM_",
            checksum: 0,
            length: core::mem::size_of::<SmbiosEntry>() as u8,
            major_version: 2,
            minor_version: 8,
            max_structure_size: 128,
            entry_point_revision: 0,
            formatted_area: [0; 5],
            intermediate_anchor: *b"_DMI_",
            intermediate_checksum: 0,
            structure_table_length: 256,
            structure_table_address: 0x12345678,
            number_of_structures: 10,
            bcd_revision: 0x28,
        };
        unsafe {
            recalculate_entry_2x_checksum(&mut entry2);
        }
        let raw2 = (&entry2 as *const SmbiosEntry).cast::<u8>();
        let mut sum2: u8 = 0;
        for i in 0..entry2.length as usize {
            sum2 = sum2.wrapping_add(unsafe { *raw2.add(i) });
        }
        assert_eq!(sum2, 0);

        let mut entry3 = Smbios3Entry {
            anchor_string: *b"_SM3_",
            checksum: 0,
            length: core::mem::size_of::<Smbios3Entry>() as u8,
            major_version: 3,
            minor_version: 6,
            docrev: 0,
            entry_point_revision: 1,
            reserved: 0,
            structure_table_max_size: 4096,
            structure_table_address: 0x0000_0001_8000_0000,
        };
        unsafe {
            recalculate_entry_3x_checksum(&mut entry3);
        }
        let raw3 = (&entry3 as *const Smbios3Entry).cast::<u8>();
        let mut sum3: u8 = 0;
        for i in 0..entry3.length as usize {
            sum3 = sum3.wrapping_add(unsafe { *raw3.add(i) });
        }
        assert_eq!(sum3, 0);
    }
}
