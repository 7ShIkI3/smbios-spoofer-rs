//! Structures et fonctions de navigation SMBIOS.
//!
//! ## Ce que tu apprends ici :
//! - `#[repr(C)]` : pourquoi c'est crucial pour le mapping mémoire
//! - `union` en Rust : un seul emplacement mémoire, plusieurs vues
//! - Pointeurs bruts (`*const`, `*mut`) et `unsafe`
//! - Pattern matching sur les types SMBIOS
//! - Comment naviguer une liste chaînée en mémoire brute

// ─── Constantes SMBIOS ─────────────────────────────────────────────────────

pub const SMBIOS_TYPE_BIOS: u8 = 0;
pub const SMBIOS_TYPE_SYSTEM: u8 = 1;
pub const SMBIOS_TYPE_BASEBOARD: u8 = 2;
pub const SMBIOS_TYPE_ENCLOSURE: u8 = 3;
pub const SMBIOS_TYPE_PROCESSOR: u8 = 4;
pub const SMBIOS_TYPE_END_OF_TABLE: u8 = 0x7F;

// GUID de la table SMBIOS dans la table de configuration UEFI
pub const SMBIOS_TABLE_GUID: uefi::Guid = uefi::Guid::new(
    [0x31, 0x2D, 0x9D, 0xEB], // time_low: 0xEB9D2D31
    [0x88, 0x2D],              // time_mid: 0x2D88
    [0xD3, 0x11],              // time_high_and_version: 0x11D3
    0x9A,                      // clock_seq_high_and_reserved
    0x16,                      // clock_seq_low
    [0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D], // node
);

pub const SMBIOS3_TABLE_GUID: uefi::Guid = uefi::Guid::new(
    [0x44, 0x15, 0xFD, 0xF2], // time_low: 0xF2FD1544
    [0x94, 0x97],              // time_mid: 0x9794
    [0x2C, 0x4A],              // time_high_and_version: 0x4A2C
    0x99,                      // clock_seq_high_and_reserved
    0x2E,                      // clock_seq_low
    [0xE5, 0xBB, 0xCF, 0x20, 0xE3, 0x94], // node
);

/// GUID du protocole EFI_SMBIOS_PROTOCOL (PI Spec 1.6+).
/// Ce protocole permettrait de modifier les chaînes SMBIOS via l'API officielle
/// `EFI_SMBIOS_UPDATE_STRING` au lieu d'écrire directement en mémoire.
///
/// Statut : GUID défini, prêt pour intégration future.
/// Complexité : nécessite wrapper FFI complet du protocole (6 pointeurs de fonction).
/// Pour l'instant, le patch direct en mémoire fonctionne sur tous les firmwares testés.
pub const SMBIOS_PROTOCOL_GUID: uefi::Guid = uefi::Guid::new(
    [0xF6, 0x83, 0x35, 0x03], // time_low: 0x03583FF6
    [0x36, 0xCB],              // time_mid: 0xCB36
    [0x40, 0x49],              // time_high_and_version: 0x4940
    0x94,                      // clock_seq_high_and_reserved
    0x7E,                      // clock_seq_low
    [0xB9, 0xB3, 0x9F, 0x4A, 0xFA, 0xF7], // node
);

/// Vérifie si le protocole EFI_SMBIOS_PROTOCOL est disponible sur ce firmware.
/// Cherche un handle qui expose le GUID SMBIOS_PROTOCOL.
/// Retourne true si le firmware supporte l'API officielle de modification SMBIOS.
pub fn smbios_protocol_available() -> bool {
    // On cherche les handles qui exposent le protocole via son GUID.
    // `find_handles` nécessite un type implémentant ProtocolPointer,
    // donc on passe par locate_handle_buffer avec le GUID brut.
    uefi::boot::locate_handle_buffer(uefi::boot::SearchType::ByProtocol(&SMBIOS_PROTOCOL_GUID))
        .is_ok()
}

// ─── Structs SMBIOS (mapping mémoire) ──────────────────────────────────────

/// Point d'entrée SMBIOS (Entry Point Structure 32-bit).
/// Trouvé via le GUID dans la table de configuration UEFI.
/// C'est la première chose qu'on cherche au boot.
#[repr(C)]        // Garantit la disposition mémoire exacte (comme en C)
#[derive(Debug)]
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
    pub structure_table_address: u32, // Adresse physique de la table
    pub number_of_structures: u16,
    pub bcd_revision: u8,
}

/// En-tête commun à toutes les structures SMBIOS.
/// Chaque entrée de la table commence par ces 4 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SmbiosHeader {
    pub type_: u8,      // Type de structure (0=BIOS, 1=System, etc.)
    pub length: u8,      // Taille de la partie formatée
    pub handle: u16,     // Identifiant unique
}

// ─── Types SMBIOS spécifiques ─────────────────────────────────────────────

/// Type 0 : BIOS Information
#[repr(C)]
pub struct SmbiosType0 {
    pub header: SmbiosHeader,
    pub vendor: u8,              // Index de chaîne
    pub bios_version: u8,        // Index de chaîne
    pub bios_start_segment: u16,
    pub bios_release_date: u8,   // Index de chaîne
    pub bios_rom_size: u8,
}

/// Type 1 : System Information — LE plus important à spoof
#[repr(C)]
pub struct SmbiosType1 {
    pub header: SmbiosHeader,
    pub manufacturer: u8,    // Index chaîne
    pub product_name: u8,    // Index chaîne
    pub version: u8,         // Index chaîne
    pub serial_number: u8,   // Index chaîne ← ON SPOOF ÇA
    pub uuid: [u8; 16],     // UUID système ← ON SPOOF ÇA
    pub wake_up_type: u8,
    pub sku_number: u8,      // Index chaîne
    pub family: u8,          // Index chaîne
}

/// Type 2 : Baseboard (Motherboard) Information
#[repr(C)]
pub struct SmbiosType2 {
    pub header: SmbiosHeader,
    pub manufacturer: u8,        // Index chaîne
    pub product_name: u8,       // Index chaîne ← Modèle carte mère
    pub version: u8,            // Index chaîne
    pub serial_number: u8,      // Index chaîne ← ON SPOOF ÇA
    pub asset_tag: u8,          // Index chaîne
    pub feature_flags: u8,
    pub location_in_chassis: u8, // Index chaîne
    pub chassis_handle: u16,
    pub board_type: u8,
}

/// Type 3 : System Enclosure / Chassis
#[repr(C)]
pub struct SmbiosType3 {
    pub header: SmbiosHeader,
    pub manufacturer: u8,          // Index chaîne
    pub enclosure_type: u8,       // Type d'enclosure (flags binaires)
    pub version: u8,              // Index chaîne
    pub serial_number: u8,        // Index chaîne ← ON SPOOF ÇA
    pub asset_tag: u8,           // Index chaîne
    pub boot_up_state: u8,
    pub power_supply_state: u8,
    pub thermal_state: u8,
    pub security_status: u8,
    pub oem_defined: u32,
    pub height: u8,
    pub number_power_cords: u8,
    // Note : la structure peut contenir des champs supplémentaires
    // (contained element count/records, SKU number). On utilise
    // header.length pour connaître la taille réelle du formatted area.
}

/// Type 4 : Processor Information
#[repr(C)]
pub struct SmbiosType4 {
    pub header: SmbiosHeader,
    pub socket_designation: u8,      // Index chaîne
    pub processor_type: u8,
    pub processor_family: u8,
    pub processor_manufacturer: u8,  // Index chaîne
    pub processor_id: u64,           // CPUID (ne peut pas être spoofé)
    pub processor_version: u8,       // Index chaîne
    pub voltage: u8,
    pub external_clock: u16,
    pub max_speed: u16,
    pub current_speed: u16,
    pub status: u8,
    pub processor_upgrade: u8,
    pub l1_cache_handle: u16,
    pub l2_cache_handle: u16,
    pub l3_cache_handle: u16,
    pub serial_number: u8,           // Index chaîne ← ON SPOOF ÇA
    pub asset_tag: u8,              // Index chaîne
}

// ─── Union pour naviguer les structures ────────────────────────────────────

/// Union qui permet d'accéder à une structure SMBIOS
/// sous n'importe quel type, comme en C avec SMBIOS_STRUCTURE_POINTER.
///
/// En Rust, une `union` c'est comme en C : tous les champs partagent
/// la même adresse mémoire. On accède via `unsafe` car le compilateur
/// ne peut pas vérifier quel champ est actif.
#[repr(C)]
pub union SmbiosStructPtr {
    pub raw: *const u8,              // Pointeur brut (accès octet par octet)
    pub header: *const SmbiosHeader, // Vu comme en-tête générique
    pub type0: *const SmbiosType0,   // Vu comme Type 0
    pub type1: *const SmbiosType1,   // Vu comme Type 1
    pub type2: *const SmbiosType2,   // Vu comme Type 2
    pub type3: *const SmbiosType3,   // Vu comme Type 3
    pub type4: *const SmbiosType4,   // Vu comme Type 4
}

// Pour rendre le type plus sûr, on wrap l'union dans une struct
// qui connaît le type effectif de la structure.
pub struct SmbiosStruct {
    pub(crate) ptr: SmbiosStructPtr,
    pub type_: u8,
}

impl SmbiosStruct {
    /// Crée depuis un pointeur brut.
    /// SAFETY: caller doit s'assurer que le pointeur pointe vers
    /// une structure SMBIOS valide.
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
        unsafe { (*self.ptr.header).length as usize }
    }

    /// Accède au header.
    pub fn header(&self) -> &SmbiosHeader {
        unsafe { &*self.ptr.header }
    }

    /// Accède au Type 1 (System).
    pub fn as_type1(&self) -> Option<&SmbiosType1> {
        if self.type_ == SMBIOS_TYPE_SYSTEM {
            unsafe { Some(&*self.ptr.type1) }
        } else {
            None
        }
    }

    /// Accède au Type 2 (Baseboard).
    pub fn as_type2(&self) -> Option<&SmbiosType2> {
        if self.type_ == SMBIOS_TYPE_BASEBOARD {
            unsafe { Some(&*self.ptr.type2) }
        } else {
            None
        }
    }

    /// Accède au Type 3 (Chassis).
    pub fn as_type3(&self) -> Option<&SmbiosType3> {
        if self.type_ == SMBIOS_TYPE_ENCLOSURE {
            unsafe { Some(&*self.ptr.type3) }
        } else {
            None
        }
    }

    /// Accède au Type 4 (Processor).
    pub fn as_type4(&self) -> Option<&SmbiosType4> {
        if self.type_ == SMBIOS_TYPE_PROCESSOR {
            unsafe { Some(&*self.ptr.type4) }
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
        // SAFETY: on s'assure que le pointeur est valide via from_raw
        unsafe { (self.ptr.raw as *mut u8).add(self.header_length()) }
    }
}

// ─── Navigation des tables SMBIOS ──────────────────────────────────────────

/// Calcule la longueur totale d'une structure (partie formatée + chaînes).
/// Équivalent de `TableLenght()` en C (oui, la faute de frappe est dans l'original).
/// La table de chaînes se termine par un double zéro.
pub unsafe fn table_length(ptr: *const u8) -> usize {
    let header = unsafe { &*ptr.cast::<SmbiosHeader>() };
    let string_start = unsafe { ptr.add(header.length as usize) };

    // Parcourir la table de chaînes jusqu'au double null
    let mut end = string_start;
    loop {
        if unsafe { *end == 0 && *end.add(1) == 0 } {
            break;
        }
        end = unsafe { end.add(1) };
    }

    // +2 pour inclure le double null terminal
    unsafe { end.offset_from(ptr) as usize + 2 }
}

/// Trouve une structure SMBIOS par type.
/// `index` = 0 pour la première occurrence, 1 pour la deuxième, etc.
/// Retourne `None` si pas trouvée.
pub fn find_by_type(entry: &SmbiosEntry, type_: u8, index: usize) -> Option<SmbiosStruct> {
    let mut current = entry.structure_table_address as *const u8;

    if current.is_null() {
        return None;
    }

    let mut type_index = 0;

    loop {
        let header = unsafe { &*current.cast::<SmbiosHeader>() };

        // Fin de table ?
        if header.type_ == SMBIOS_TYPE_END_OF_TABLE {
            return None;
        }

        // Trouvé ?
        if header.type_ == type_ {
            if type_index == index {
                return unsafe { SmbiosStruct::from_raw(current) };
            }
            type_index += 1;
        }

        // Structure suivante
        let len = unsafe { table_length(current) };
        current = unsafe { current.add(len) };
    }
}

// ─── Édition des chaînes SMBIOS ────────────────────────────────────────────

/// Remplace une chaîne dans la table SMBIOS.
/// Équivalent de `EditString()` en C.
///
/// CONTRAINTE : la nouvelle chaîne doit être ≤ l'ancienne en longueur.
/// On ne peut pas agrandir car on écrase en place.
///
/// # Safety
/// Le pointeur `table_ptr` doit être valide et pointer vers une structure SMBIOS.
pub unsafe fn edit_string(
    table_ptr: *mut u8,
    header_len: usize,
    field_index: u8,
    new_value: &[u8],
) {
    if field_index == 0 || new_value.is_empty() {
        return;
    }

    // Naviguer jusqu'à la chaîne cible dans la table de chaînes
    let mut astr = unsafe { table_ptr.add(header_len) };
    let mut index: u8 = 1;

    while index != field_index {
        if unsafe { *astr != 0 } {
            index += 1;
        }
        // Avancer jusqu'au null
        while unsafe { *astr != 0 } {
            astr = unsafe { astr.add(1) };
        }
        astr = unsafe { astr.add(1) };

        // Fin de table de chaînes (double null)
        if unsafe { *astr == 0 && *astr.add(1) == 0 } {
            break;
        }
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

    // Copier la nouvelle valeur (tronquée si trop longue)
    let copy_len = new_value.len().min(old_len);
    for i in 0..copy_len {
        unsafe { *astr.add(i) = new_value[i]; }
    }
    // Pad avec des espaces si la nouvelle valeur est plus courte
    for i in copy_len..old_len {
        unsafe { *astr.add(i) = b' '; }
    }
}

// ─── Lecture des chaînes SMBIOS ────────────────────────────────────────────

/// Lit une chaîne depuis une structure SMBIOS.
/// Convertit de l'ASCII/UTF-8 en UTF-16 UEFI.
pub unsafe fn read_smbios_string(
    table_ptr: *const u8,
    header_len: usize,
    field_index: u8,
    output: &mut [u16],
) {
    if field_index == 0 {
        output[0] = 0;
        return;
    }

    let mut astr = unsafe { table_ptr.add(header_len) };
    let mut index: u8 = 1;

    while index != field_index {
        if unsafe { *astr != 0 } {
            index += 1;
        }
        while unsafe { *astr != 0 } {
            astr = unsafe { astr.add(1) };
        }
        astr = unsafe { astr.add(1) };

        if unsafe { *astr == 0 && *astr.add(1) == 0 } {
            output[0] = 0;
            return;
        }
    }

    let max = output.len() - 1;
    for i in 0..max {
        let ch = unsafe { *astr.add(i) };
        output[i] = ch as u16;
        if ch == 0 {
            break;
        }
    }
}
