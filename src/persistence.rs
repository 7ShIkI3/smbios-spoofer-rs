//! Persistance NVRAM et disque.
//!
//! ## Ce que tu apprends ici :
//! - `#[repr(C, packed)]` pour les structures sérialisées
//! - Manipulation des variables NVRAM UEFI (get_variable/set_variable)
//! - Accès au système de fichiers UEFI (Simple File System)
//! - Checksum pour l'intégrité des données
//! - Gestion d'erreur avec `Result<(), Error>` plutôt que des codes de retour

use uefi::cstr16;
use uefi::proto::media::file::{File, FileAttribute, FileMode, RegularFile};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::runtime::{VariableAttributes, VariableVendor};
use uefi::{CStr16, Guid};
use core::mem;

use crate::utils;

// ─── Constantes ────────────────────────────────────────────────────────────

/// GUID pour la variable NVRAM
const NVRAM_VENDOR_GUID: Guid = Guid::new(
    [0x61, 0xDF, 0xE4, 0x8B], // time_low: 0x8BE4DF61
    [0xCA, 0x93],              // time_mid: 0x93CA
    [0xD2, 0x11],              // time_high_and_version: 0x11D2
    0xAA,                      // clock_seq_high_and_reserved
    0x0D,                      // clock_seq_low
    [0x00, 0xE0, 0x98, 0x03, 0x2B, 0x8C], // node
);

const NVRAM_VARIABLE_NAME: &CStr16 = cstr16!("SmbiosSpoof");
const NVRAM_MAGIC: u32 = 0x534D4249; // "SMBI" en big-endian
const NVRAM_VERSION: u32 = 6; // v6: ajout du chassis_serial (Type 3)

const DISK_DIR: &CStr16 = cstr16!("\\EFI\\SmbiosSpoofer");
const DISK_FILE: &CStr16 = cstr16!("\\EFI\\SmbiosSpoofer\\uuid.dat");

// ─── Structure de données persistée ────────────────────────────────────────

/// Structure sauvegardée en NVRAM et sur disque.
/// Contient toutes les valeurs spoofées + checksum d'intégrité.
///
/// `#[repr(C, packed)]` garantit :
/// - Disposition mémoire identique à la version C (interopérabilité)
/// - Pas de padding entre les champs (taille exacte prévisible)
///
/// NOTE: Les champs de type `[u16; 64]` dans une struct packed créent des
/// références non-alignées. On utilise des copies locales pour éviter les UB.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct SpoofConfig {
    pub magic: u32,              // 0x534D4249 ("SMBI")
    pub version: u32,            // Version de la structure
    pub uuid: [u8; 16],         // UUID spoofée
    pub system_serial: [u16; 64], // Serial système (Type 1)
    pub bios_serial: [u16; 64],   // Serial BIOS (Type 0)
    pub baseboard_serial: [u16; 64], // Serial carte mère (Type 2)
    pub baseboard_model: [u16; 64],  // Modèle carte mère (Type 2)
    pub processor_serial: [u16; 64], // Serial processeur (Type 4)
    pub chassis_serial: [u16; 64],   // Serial châssis (Type 3)
    pub checksum: u32,           // Checksum d'intégrité
}

impl Default for SpoofConfig {
    fn default() -> Self {
        Self {
            magic: 0,
            version: 0,
            uuid: [0u8; 16],
            system_serial: [0u16; 64],
            bios_serial: [0u16; 64],
            baseboard_serial: [0u16; 64],
            baseboard_model: [0u16; 64],
            processor_serial: [0u16; 64],
            chassis_serial: [0u16; 64],
            checksum: 0,
        }
    }
}

impl SpoofConfig {
    /// Vérifie si la config est valide (magic + checksum).
    pub fn is_valid(&self) -> bool {
        if self.magic != NVRAM_MAGIC || self.version < 3 {
            return false;
        }
        let saved = self.checksum;
        let mut copy = *self;
        copy.checksum = 0;
        let calc = calculate_checksum(&copy);
        saved == calc
    }

    /// Remplit avec des valeurs fraîchement générées.
    pub fn generate(&mut self) {
        self.magic = NVRAM_MAGIC;
        self.version = NVRAM_VERSION;
        utils::generate_uuid(&mut self.uuid);

        // Copier vers des buffers locaux pour éviter les refs non-alignées sur packed struct
        let mut sys_buf = self.system_serial;
        utils::generate_serial(&mut sys_buf, 14);
        self.system_serial = sys_buf;

        let mut bb_buf = self.baseboard_serial;
        utils::generate_serial(&mut bb_buf, 14);
        self.baseboard_serial = bb_buf;

        let mut ch_buf = self.chassis_serial;
        utils::generate_serial(&mut ch_buf, 14);
        self.chassis_serial = ch_buf;

        self.bios_serial[0] = 0;
        self.processor_serial[0] = 0;
        self.checksum = 0;
        self.checksum = calculate_checksum(self);
    }

    /// Génère en imitant le format des originaux.
    /// NOTE: On reçoit des slices `&[u16]` (références vers des buffers non-packed).
    pub fn generate_with_originals(
        &mut self,
        orig_system: &[u16],
        orig_baseboard: &[u16],
        orig_processor: &[u16],
        orig_chassis: &[u16],
    ) {
        self.magic = NVRAM_MAGIC;
        self.version = NVRAM_VERSION;
        utils::generate_uuid(&mut self.uuid);

        // Copier les champs packed vers des locaux avant de passer des refs
        // pour éviter les warnings d'alignement
        let mut system_buf = [0u16; 64];
        system_buf.copy_from_slice(orig_system);
        let mut baseboard_buf = [0u16; 64];
        baseboard_buf.copy_from_slice(orig_baseboard);
        let mut processor_buf = [0u16; 64];
        processor_buf.copy_from_slice(orig_processor);
        let mut chassis_buf = [0u16; 64];
        chassis_buf.copy_from_slice(orig_chassis);

        // Génération intelligente : détecte le constructeur si possible
        let mut sys_serial = [0u16; 64];
        utils::generate_serial_smart(&mut sys_serial, &system_buf);
        self.system_serial = sys_serial;

        let mut bb_serial = [0u16; 64];
        utils::generate_serial_smart(&mut bb_serial, &baseboard_buf);
        self.baseboard_serial = bb_serial;

        let mut proc_serial = [0u16; 64];
        utils::generate_serial_matching_format(&mut proc_serial, &processor_buf);
        self.processor_serial = proc_serial;

        let mut ch_serial = [0u16; 64];
        utils::generate_serial_smart(&mut ch_serial, &chassis_buf);
        self.chassis_serial = ch_serial;

        self.bios_serial[0] = 0;
        self.checksum = 0;
        self.checksum = calculate_checksum(self);
    }
}

/// Calcule le checksum (somme de tous les bytes sauf le champ checksum).
fn calculate_checksum(config: &SpoofConfig) -> u32 {
    let ptr = config as *const SpoofConfig as *const u8;
    let size = mem::size_of::<SpoofConfig>() - mem::size_of::<u32>();
    let mut sum: u32 = 0;
    for i in 0..size {
        sum = sum.wrapping_add(unsafe { *ptr.add(i) } as u32);
    }
    sum
}

// ─── Persistance NVRAM ─────────────────────────────────────────────────────

/// Sauvegarde la config en NVRAM.
pub fn save_to_nvram(config: &SpoofConfig) -> Result<(), uefi::Error> {
    let data = unsafe {
        core::slice::from_raw_parts(
            config as *const SpoofConfig as *const u8,
            mem::size_of::<SpoofConfig>(),
        )
    };

    let attrs = VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::NON_VOLATILE;
    let vendor = VariableVendor(NVRAM_VENDOR_GUID);

    uefi::runtime::set_variable(NVRAM_VARIABLE_NAME, &vendor, attrs, data)
}

/// Charge la config depuis la NVRAM.
pub fn load_from_nvram() -> Result<SpoofConfig, uefi::Error> {
    let mut config = SpoofConfig::default();
    let data = unsafe {
        core::slice::from_raw_parts_mut(
            &mut config as *mut SpoofConfig as *mut u8,
            mem::size_of::<SpoofConfig>(),
        )
    };

    let vendor = VariableVendor(NVRAM_VENDOR_GUID);
    let (_data, _attrs) = uefi::runtime::get_variable(NVRAM_VARIABLE_NAME, &vendor, data)
        .map_err(|e| uefi::Error::new(e.status(), ()))?;

    if config.is_valid() {
        Ok(config)
    } else {
        Err(uefi::Error::new(uefi::Status::CRC_ERROR, ()))
    }
}

// ─── Persistance disque ────────────────────────────────────────────────────

/// Sauvegarde la config sur le disque (partition EFI).
pub fn save_to_disk(config: &SpoofConfig) -> Result<(), uefi::Error> {
    let handle = uefi::boot::get_handle_for_protocol::<SimpleFileSystem>()?;
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume()?;

    // Essayer de créer le fichier (crée aussi le dossier intermédiaire si possible)
    let file_handle = match root.open(DISK_FILE, FileMode::CreateReadWrite, FileAttribute::empty()) {
        Ok(fh) => fh,
        Err(_) => {
            // Si l'ouverture échoue, le dossier n'existe peut-être pas.
            // On tente de créer le dossier d'abord (via open sur le dossier parent),
            // puis on réessaie.
            // Note: en FAT UEFI, on ne peut pas créer de dossiers arbitraires.
            // On ignore l'erreur — le dossier est créé par l'utilisateur
            // lors de l'installation du .efi.
            return Err(uefi::Error::new(uefi::Status::NOT_FOUND, ()));
        }
    };

    let mut file: RegularFile = file_handle.into_regular_file()
        .ok_or_else(|| uefi::Error::new(uefi::Status::DEVICE_ERROR, ()))?;

    let data = unsafe {
        core::slice::from_raw_parts(
            config as *const SpoofConfig as *const u8,
            mem::size_of::<SpoofConfig>(),
        )
    };

    file.write(data).map_err(|_| uefi::Error::new(uefi::Status::DEVICE_ERROR, ()))?;
    file.flush()?;

    Ok(())
}

/// Charge la config depuis le disque.
pub fn load_from_disk() -> Result<SpoofConfig, uefi::Error> {
    let handle = uefi::boot::get_handle_for_protocol::<SimpleFileSystem>()?;
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume()?;

    let file_handle = root.open(DISK_FILE, FileMode::Read, FileAttribute::empty())?;
    let mut file: RegularFile = file_handle.into_regular_file()
        .ok_or_else(|| uefi::Error::new(uefi::Status::DEVICE_ERROR, ()))?;

    let mut config = SpoofConfig::default();
    let data = unsafe {
        core::slice::from_raw_parts_mut(
            &mut config as *mut SpoofConfig as *mut u8,
            mem::size_of::<SpoofConfig>(),
        )
    };

    let _ = file.read(data)?;

    if config.is_valid() {
        Ok(config)
    } else {
        Err(uefi::Error::new(uefi::Status::CRC_ERROR, ()))
    }
}

/// Essaie de charger : NVRAM d'abord, puis disque en fallback.
pub fn load_config() -> Option<SpoofConfig> {
    load_from_nvram()
        .or_else(|_| load_from_disk())
        .ok()
}

/// Sauvegarde dans les deux (NVRAM + disque) — best effort.
pub fn save_config(config: &SpoofConfig) {
    let _ = save_to_nvram(config);
    let _ = save_to_disk(config);
}
