//! Persistance multi-couche : NVRAM UEFI + partition EFI (ESP `\EFI\SmbiosSpoofer\uuid.dat`).
//!
//! Améliorations v8 :
//! - Support de tous les volumes `SimpleFileSystem` (recherche de la partition ESP contenant `\EFI`)
//! - Création automatique du dossier `\EFI\SmbiosSpoofer` s'il n'existe pas
//! - Fallback attributs NVRAM (`RUNTIME_ACCESS` + `BOOTSERVICE_ACCESS`, puis `BOOTSERVICE_ACCESS` seul)
//! - Nouveaux champs persistés : `chassis_asset_tag`, `baseboard_asset_tag`, `system_sku`,
//!   `ram_serial_seed` (pour le spoofing déterministe des barrettes RAM Type 17) et `oem_seed` (Type 11)
//! - Upgrade transparent des profils v3..v7 vers v8

#![cfg_attr(not(target_os = "uefi"), allow(dead_code))]

use core::mem;
use uefi::{CStr16, Guid, cstr16};

#[cfg(target_os = "uefi")]
use uefi::Identify;
#[cfg(target_os = "uefi")]
use uefi::proto::media::file::{File, FileAttribute, FileMode, RegularFile};
#[cfg(target_os = "uefi")]
use uefi::proto::media::fs::SimpleFileSystem;
#[cfg(target_os = "uefi")]
use uefi::runtime::{VariableAttributes, VariableVendor};

use crate::patch::OriginalSmbiosData;
use crate::utils;

// ─── Constantes ────────────────────────────────────────────────────────────

/// GUID historique pour la variable NVRAM
const NVRAM_VENDOR_GUID: Guid = Guid::new(
    [0x61, 0xDF, 0xE4, 0x8B], // time_low: 0x8BE4DF61
    [0xCA, 0x93],             // time_mid: 0x93CA
    [0xD2, 0x11],             // time_high_and_version: 0x11D2
    0xAA,                     // clock_seq_high_and_reserved
    0x0D,                     // clock_seq_low
    [0x00, 0xE0, 0x98, 0x03, 0x2B, 0x8C],
);

pub const NVRAM_VARIABLE_NAME: &CStr16 = cstr16!("SmbiosSpoof");
pub const NVRAM_MAGIC: u32 = 0x534D4249; // "SMBI"
pub const NVRAM_VERSION: u32 = 9; // v9: +disk_serial, +disk_serial_seed (v8: +asset_tags, +system_sku, +ram_serial_seed, +oem_seed)

pub const EFI_ROOT_DIR: &CStr16 = cstr16!("\\EFI");
pub const DISK_DIR: &CStr16 = cstr16!("\\EFI\\SmbiosSpoofer");
pub const DISK_FILE: &CStr16 = cstr16!("\\EFI\\SmbiosSpoofer\\uuid.dat");

/// État de sauvegarde retourné à l'interface graphique.
#[derive(Debug, Clone, Copy, Default)]
pub struct PersistenceStatus {
    pub nvram_ok: bool,
    pub disk_ok: bool,
}

// ─── Structure de données persistée ────────────────────────────────────────

/// Structure sauvegardée en NVRAM et sur disque (`\EFI\SmbiosSpoofer\uuid.dat`).
///
/// `#[repr(C, packed)]` garantit une disposition binaire stable sans padding.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct SpoofConfig {
    pub magic: u32,                  // 0x534D4249 ("SMBI")
    pub version: u32,                // Version de la structure (9)
    pub uuid: [u8; 16],              // UUID système (Type 1)
    pub system_serial: [u16; 64],    // Serial système (Type 1)
    pub bios_serial: [u16; 64],      // Réservé / Serial BIOS
    pub baseboard_serial: [u16; 64], // Serial carte mère (Type 2)
    pub baseboard_model: [u16; 64],  // Modèle carte mère (Type 2)
    pub processor_serial: [u16; 64], // Serial processeur (Type 4)
    pub chassis_serial: [u16; 64],   // Serial châssis (Type 3)
    pub bios_vendor: [u16; 64],      // Vendor BIOS (Type 0) — v7+
    pub bios_version: [u16; 64],     // Version BIOS (Type 0) — v7+
    pub bios_date: [u16; 64],        // Date BIOS (Type 0) — v7+
    pub mac_address: [u8; 6],        // MAC spoofée — v7+
    // Nouveaux champs v8 (placés avant checksum) :
    pub chassis_asset_tag: [u16; 64],   // Asset Tag châssis (Type 3) — v8+
    pub baseboard_asset_tag: [u16; 64], // Asset Tag carte mère (Type 2) — v8+
    pub system_sku: [u16; 64],          // SKU Number système (Type 1) — v8+
    pub ram_serial_seed: u32,           // Graine déterministe pour les barrettes RAM (Type 17) — v8+
    pub oem_seed: u32,                  // Graine déterministe pour les chaînes OEM (Type 11) — v8+
    // Nouveaux champs v9 (Disk Spoofing) :
    pub disk_serial: [u16; 64],         // Serial disque principal (NVMe / SATA) — v9+
    pub disk_serial_seed: u32,          // Graine déterministe pour tous les disques — v9+
    pub checksum: u32,                  // Checksum d'intégrité (somme des octets précédents)
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
            bios_vendor: [0u16; 64],
            bios_version: [0u16; 64],
            bios_date: [0u16; 64],
            mac_address: [0u8; 6],
            chassis_asset_tag: [0u16; 64],
            baseboard_asset_tag: [0u16; 64],
            system_sku: [0u16; 64],
            ram_serial_seed: 0,
            oem_seed: 0,
            disk_serial: [0u16; 64],
            disk_serial_seed: 0,
            checksum: 0,
        }
    }
}

impl SpoofConfig {
    /// Vérifie si la config est valide (magic + version courante + checksum).
    pub fn is_valid(&self) -> bool {
        if self.magic != NVRAM_MAGIC || self.version != NVRAM_VERSION {
            return false;
        }
        let saved = self.checksum;
        let mut copy = *self;
        copy.checksum = 0;
        let calc = calculate_checksum(&copy);
        saved == calc
    }

    /// Détecte si la config provient d'une version antérieure (v3..v7) et doit être migrée.
    pub fn needs_upgrade(&self) -> bool {
        self.magic == NVRAM_MAGIC && self.version >= 3 && self.version < NVRAM_VERSION
    }

    /// Remplit avec des valeurs génériques fraîchement générées.
    pub fn generate(&mut self) {
        self.magic = NVRAM_MAGIC;
        self.version = NVRAM_VERSION;
        utils::generate_uuid(&mut self.uuid);

        let mut sys_buf = [0u16; 64];
        utils::generate_serial(&mut sys_buf, 14);
        self.system_serial = sys_buf;

        let mut bb_buf = [0u16; 64];
        utils::generate_serial(&mut bb_buf, 14);
        self.baseboard_serial = bb_buf;

        let mut ch_buf = [0u16; 64];
        utils::generate_serial(&mut ch_buf, 14);
        self.chassis_serial = ch_buf;

        let mut proc_buf = [0u16; 64];
        utils::generate_serial(&mut proc_buf, 16);
        self.processor_serial = proc_buf;

        self.bios_serial = [0u16; 64];

        let mut bv_buf = [0u16; 64];
        utils::generate_bios_vendor(&mut bv_buf);
        self.bios_vendor = bv_buf;

        let mut bver_buf = [0u16; 64];
        utils::generate_bios_version(&mut bver_buf);
        self.bios_version = bver_buf;

        let mut bd_buf = [0u16; 64];
        utils::generate_bios_date(&mut bd_buf);
        self.bios_date = bd_buf;

        let mut cat_buf = [0u16; 64];
        utils::generate_serial(&mut cat_buf, 10);
        self.chassis_asset_tag = cat_buf;

        let mut bat_buf = [0u16; 64];
        utils::generate_serial(&mut bat_buf, 10);
        self.baseboard_asset_tag = bat_buf;

        let mut sku_buf = [0u16; 64];
        utils::generate_serial(&mut sku_buf, 10);
        self.system_sku = sku_buf;

        self.ram_serial_seed = utils::random_u32() | 1;
        self.oem_seed = utils::random_u32() | 1;
        self.disk_serial_seed = utils::random_u32() | 1;

        let mut disk_buf = [0u16; 64];
        utils::generate_disk_serial_deterministic(&mut disk_buf, &[0u16; 64], self.disk_serial_seed, 0);
        self.disk_serial = disk_buf;

        #[cfg(feature = "spoof-mac")]
        {
            let mut mac_buf = [0u8; 6];
            utils::generate_mac_address(&mut mac_buf);
            self.mac_address = mac_buf;
        }

        self.checksum = 0;
        self.checksum = calculate_checksum(self);
    }

    /// Génère un profil complet en imitant fidèlement les formats et longueurs
    /// de toutes les données matérielles originales ([`OriginalSmbiosData`]).
    pub fn generate_from_originals(&mut self, orig: &OriginalSmbiosData) {
        self.magic = NVRAM_MAGIC;
        self.version = NVRAM_VERSION;
        utils::generate_uuid(&mut self.uuid);

        let vendor_hint = utils::detect_vendor_by_name(&orig.system_manufacturer)
            .or_else(|| utils::detect_vendor_by_name(&orig.baseboard_manufacturer))
            .or_else(|| utils::detect_vendor_by_name(&orig.bios_vendor));

        let mut sys_serial = [0u16; 64];
        if let Some(vendor) = vendor_hint {
            let orig_len = utils::str_len(&orig.system_serial);
            if orig_len == 0 || utils::is_placeholder(&orig.system_serial) {
                utils::generate_serial_for_vendor(&mut sys_serial, vendor, 12);
            } else {
                utils::generate_serial_smart(&mut sys_serial, &orig.system_serial);
            }
        } else {
            utils::generate_serial_smart(&mut sys_serial, &orig.system_serial);
        }
        self.system_serial = sys_serial;

        let mut bb_serial = [0u16; 64];
        utils::generate_serial_smart(&mut bb_serial, &orig.baseboard_serial);
        self.baseboard_serial = bb_serial;

        let mut proc_serial = [0u16; 64];
        utils::generate_serial_matching_format(&mut proc_serial, &orig.processor_serial);
        self.processor_serial = proc_serial;

        let mut ch_serial = [0u16; 64];
        utils::generate_serial_smart(&mut ch_serial, &orig.chassis_serial);
        self.chassis_serial = ch_serial;

        self.bios_serial = [0u16; 64];
        self.baseboard_model = orig.baseboard_model;

        // Conserver le vrai vendor BIOS s'il est valide (plus furtif), sinon en générer un
        let mut bv_buf = [0u16; 64];
        if orig.bios_vendor[0] != 0 && !utils::is_placeholder(&orig.bios_vendor) {
            bv_buf = orig.bios_vendor;
        } else {
            utils::generate_bios_vendor(&mut bv_buf);
        }
        self.bios_vendor = bv_buf;

        let mut bver_buf = [0u16; 64];
        utils::generate_bios_version_like(&mut bver_buf, &orig.bios_version);
        self.bios_version = bver_buf;

        let mut bd_buf = [0u16; 64];
        utils::generate_bios_date_like(&mut bd_buf, &orig.bios_date);
        self.bios_date = bd_buf;

        let mut cat_buf = [0u16; 64];
        utils::generate_serial_matching_format(&mut cat_buf, &orig.chassis_asset_tag);
        self.chassis_asset_tag = cat_buf;

        let mut bat_buf = [0u16; 64];
        utils::generate_serial_matching_format(&mut bat_buf, &orig.baseboard_asset_tag);
        self.baseboard_asset_tag = bat_buf;

        let mut sku_buf = [0u16; 64];
        utils::generate_serial_matching_format(&mut sku_buf, &orig.system_sku);
        self.system_sku = sku_buf;

        self.ram_serial_seed = utils::random_u32() | 1;
        self.oem_seed = utils::random_u32() | 1;
        self.disk_serial_seed = utils::random_u32() | 1;

        let mut disk_buf = [0u16; 64];
        utils::generate_disk_serial_deterministic(
            &mut disk_buf,
            &orig.first_disk_serial,
            self.disk_serial_seed,
            0,
        );
        self.disk_serial = disk_buf;

        #[cfg(feature = "spoof-mac")]
        {
            let mut mac_buf = [0u8; 6];
            utils::generate_mac_address(&mut mac_buf);
            self.mac_address = mac_buf;
        }

        self.checksum = 0;
        self.checksum = calculate_checksum(self);
    }

    /// Compatibilité avec l'ancienne signature à 4 slices.
    pub fn generate_with_originals(
        &mut self,
        orig_system: &[u16],
        orig_baseboard: &[u16],
        orig_processor: &[u16],
        orig_chassis: &[u16],
    ) {
        let mut orig = OriginalSmbiosData::default();
        let copy_slice = |dst: &mut [u16; 64], src: &[u16]| {
            let n = src.len().min(63);
            dst[..n].copy_from_slice(&src[..n]);
            dst[n] = 0;
        };
        copy_slice(&mut orig.system_serial, orig_system);
        copy_slice(&mut orig.baseboard_serial, orig_baseboard);
        copy_slice(&mut orig.processor_serial, orig_processor);
        copy_slice(&mut orig.chassis_serial, orig_chassis);
        self.generate_from_originals(&orig);
    }

    /// Migre une configuration v3..v7 vers la version v8 courante en préservant
    /// l'UUID et les serials déjà générés.
    pub fn upgrade_from_older(&mut self) {
        if self.bios_vendor[0] == 0 {
            let mut buf = [0u16; 64];
            utils::generate_bios_vendor(&mut buf);
            self.bios_vendor = buf;
        }
        if self.bios_version[0] == 0 {
            let mut buf = [0u16; 64];
            utils::generate_bios_version(&mut buf);
            self.bios_version = buf;
        }
        if self.bios_date[0] == 0 {
            let mut buf = [0u16; 64];
            utils::generate_bios_date(&mut buf);
            self.bios_date = buf;
        }
        if self.chassis_asset_tag[0] == 0 {
            let mut buf = [0u16; 64];
            utils::generate_serial(&mut buf, 10);
            self.chassis_asset_tag = buf;
        }
        if self.baseboard_asset_tag[0] == 0 {
            let mut buf = [0u16; 64];
            utils::generate_serial(&mut buf, 10);
            self.baseboard_asset_tag = buf;
        }
        if self.system_sku[0] == 0 {
            let mut buf = [0u16; 64];
            utils::generate_serial(&mut buf, 10);
            self.system_sku = buf;
        }
        if self.ram_serial_seed == 0 {
            self.ram_serial_seed = utils::random_u32() | 1;
        }
        if self.oem_seed == 0 {
            self.oem_seed = utils::random_u32() | 1;
        }
        if self.disk_serial_seed == 0 {
            self.disk_serial_seed = utils::random_u32() | 1;
        }
        if self.disk_serial[0] == 0 {
            let mut buf = [0u16; 64];
            utils::generate_disk_serial_deterministic(&mut buf, &[0u16; 64], self.disk_serial_seed, 0);
            self.disk_serial = buf;
        }

        #[cfg(feature = "spoof-mac")]
        if self.mac_address == [0u8; 6] {
            let mut mac_buf = [0u8; 6];
            utils::generate_mac_address(&mut mac_buf);
            self.mac_address = mac_buf;
        }

        self.version = NVRAM_VERSION;
        self.checksum = 0;
        self.checksum = calculate_checksum(self);
    }

    /// Vérifie et répare les champs d'un profil chargé (notamment après upgrade v7 → v8)
    /// pour que `bios_vendor`, `bios_version` et `bios_date` aient exactement la longueur
    /// et le format des valeurs matérielles originales (évite p.ex. `"A.39"` sur un BIOS ASUS `"FX707ZM.315"`).
    /// Retourne `true` si un champ a été ajusté.
    pub fn repair_with_originals(&mut self, orig: &OriginalSmbiosData) -> bool {
        let mut modified = false;

        let orig_bver_len = utils::str_len(&orig.bios_version);
        let curr_bver = self.bios_version;
        if orig_bver_len > 0 && utils::str_len(&curr_bver) != orig_bver_len {
            let mut bver_buf = [0u16; 64];
            utils::generate_bios_version_like(&mut bver_buf, &orig.bios_version);
            self.bios_version = bver_buf;
            modified = true;
        }

        let orig_bv_len = utils::str_len(&orig.bios_vendor);
        let curr_bv = self.bios_vendor;
        if orig_bv_len > 0 && utils::str_len(&curr_bv) != orig_bv_len {
            self.bios_vendor = orig.bios_vendor;
            modified = true;
        }

        let orig_bd_len = utils::str_len(&orig.bios_date);
        let curr_bd = self.bios_date;
        if orig_bd_len > 0 && utils::str_len(&curr_bd) != orig_bd_len {
            let mut bd_buf = [0u16; 64];
            utils::generate_bios_date_like(&mut bd_buf, &orig.bios_date);
            self.bios_date = bd_buf;
            modified = true;
        }

        let orig_disk_len = utils::str_len(&orig.first_disk_serial).min(20);
        let curr_disk = self.disk_serial;
        if orig_disk_len > 0 && utils::str_len(&curr_disk) != orig_disk_len {
            let seed = if self.disk_serial_seed == 0 {
                let s = utils::random_u32() | 1;
                self.disk_serial_seed = s;
                s
            } else {
                self.disk_serial_seed
            };
            let mut disk_buf = [0u16; 64];
            utils::generate_disk_serial_deterministic(
                &mut disk_buf,
                &orig.first_disk_serial,
                seed,
                0,
            );
            self.disk_serial = disk_buf;
            modified = true;
        }

        if modified {
            self.checksum = 0;
            self.checksum = calculate_checksum(self);
        }
        modified
    }
}

/// Calcule le checksum (somme de tous les octets de `SpoofConfig` sauf le champ `checksum` final).
pub fn calculate_checksum(config: &SpoofConfig) -> u32 {
    let ptr = (config as *const SpoofConfig).cast::<u8>();
    let size = mem::size_of::<SpoofConfig>() - mem::size_of::<u32>();
    let mut sum: u32 = 0;
    for i in 0..size {
        // SAFETY: `i < size_of::<SpoofConfig>()`
        sum = sum.wrapping_add(unsafe { *ptr.add(i) } as u32);
    }
    sum
}

// ─── Persistance NVRAM ─────────────────────────────────────────────────────

/// Sauvegarde la configuration en NVRAM UEFI avec fallback d'attributs et de GUID.
#[cfg(target_os = "uefi")]
pub fn save_to_nvram(config: &SpoofConfig) -> Result<(), uefi::Error> {
    let data = unsafe {
        core::slice::from_raw_parts(
            (config as *const SpoofConfig).cast::<u8>(),
            mem::size_of::<SpoofConfig>(),
        )
    };

    let attrs_rt = VariableAttributes::NON_VOLATILE
        | VariableAttributes::BOOTSERVICE_ACCESS
        | VariableAttributes::RUNTIME_ACCESS;
    let attrs_bs = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS;

    let custom_vendor = VariableVendor(NVRAM_VENDOR_GUID);

    // 1. Essayer avec notre Vendor GUID + RUNTIME_ACCESS (standard UEFI)
    if uefi::runtime::set_variable(NVRAM_VARIABLE_NAME, &custom_vendor, attrs_rt, data).is_ok() {
        return Ok(());
    }
    // 2. Fallback sans RUNTIME_ACCESS
    if uefi::runtime::set_variable(NVRAM_VARIABLE_NAME, &custom_vendor, attrs_bs, data).is_ok() {
        return Ok(());
    }
    // 3. Fallback avec GLOBAL_VARIABLE GUID (certains firmwares filtrent les Vendor GUIDs inconnus)
    uefi::runtime::set_variable(
        NVRAM_VARIABLE_NAME,
        &VariableVendor::GLOBAL_VARIABLE,
        attrs_rt,
        data,
    )
}

#[cfg(not(target_os = "uefi"))]
pub fn save_to_nvram(_config: &SpoofConfig) -> Result<(), uefi::Error> {
    Err(uefi::Error::new(uefi::Status::UNSUPPORTED, ()))
}

/// Charge la configuration depuis la NVRAM UEFI.
#[cfg(target_os = "uefi")]
pub fn load_from_nvram() -> Result<SpoofConfig, uefi::Error> {
    let mut config = SpoofConfig::default();
    let data = unsafe {
        core::slice::from_raw_parts_mut(
            (&mut config as *mut SpoofConfig).cast::<u8>(),
            mem::size_of::<SpoofConfig>(),
        )
    };

    let custom_vendor = VariableVendor(NVRAM_VENDOR_GUID);
    let mut ok = uefi::runtime::get_variable(NVRAM_VARIABLE_NAME, &custom_vendor, data).is_ok();
    if !ok {
        ok = uefi::runtime::get_variable(
            NVRAM_VARIABLE_NAME,
            &VariableVendor::GLOBAL_VARIABLE,
            data,
        )
        .is_ok();
    }

    if !ok {
        return Err(uefi::Error::new(uefi::Status::NOT_FOUND, ()));
    }

    if config.is_valid() {
        return Ok(config);
    }

    if config.needs_upgrade() {
        config.upgrade_from_older();
        let _ = save_to_nvram(&config);
        return Ok(config);
    }

    Err(uefi::Error::new(uefi::Status::CRC_ERROR, ()))
}

#[cfg(not(target_os = "uefi"))]
pub fn load_from_nvram() -> Result<SpoofConfig, uefi::Error> {
    Err(uefi::Error::new(uefi::Status::UNSUPPORTED, ()))
}

// ─── Persistance disque (ESP) ──────────────────────────────────────────────

/// Tente d'écrire `config` sur un volume `SimpleFileSystem` donné.
#[cfg(target_os = "uefi")]
fn try_save_on_volume(
    handle: uefi::Handle,
    config: &SpoofConfig,
    require_efi_dir: bool,
) -> Result<(), uefi::Error> {
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume()?;

    if require_efi_dir {
        if root
            .open(EFI_ROOT_DIR, FileMode::Read, FileAttribute::empty())
            .is_err()
        {
            return Err(uefi::Error::new(uefi::Status::NOT_FOUND, ()));
        }
    } else {
        // S'assurer que \EFI existe (le créer si besoin)
        let _ = root.open(
            EFI_ROOT_DIR,
            FileMode::CreateReadWrite,
            FileAttribute::DIRECTORY,
        );
    }

    // Créer ou ouvrir le dossier \EFI\SmbiosSpoofer
    let _ = root.open(
        DISK_DIR,
        FileMode::CreateReadWrite,
        FileAttribute::DIRECTORY,
    );

    // Ouvrir ou créer \EFI\SmbiosSpoofer\uuid.dat
    let file_handle = root.open(DISK_FILE, FileMode::CreateReadWrite, FileAttribute::empty())?;
    let mut file: RegularFile = file_handle
        .into_regular_file()
        .ok_or_else(|| uefi::Error::new(uefi::Status::DEVICE_ERROR, ()))?;

    let data = unsafe {
        core::slice::from_raw_parts(
            (config as *const SpoofConfig).cast::<u8>(),
            mem::size_of::<SpoofConfig>(),
        )
    };

    file.write(data)
        .map_err(|_| uefi::Error::new(uefi::Status::DEVICE_ERROR, ()))?;
    file.flush()?;
    Ok(())
}

/// Sauvegarde la configuration sur la partition EFI System Partition (ESP).
/// Parcourt tous les volumes `SimpleFileSystem` en priorisant ceux qui contiennent `\EFI`.
#[cfg(target_os = "uefi")]
pub fn save_to_disk(config: &SpoofConfig) -> Result<(), uefi::Error> {
    if let Ok(handles) = uefi::boot::locate_handle_buffer(uefi::boot::SearchType::ByProtocol(
        &SimpleFileSystem::GUID,
    )) {
        // Passe 1 : volumes possédant déjà un dossier \EFI (vraie partition ESP)
        for &handle in handles.iter() {
            if try_save_on_volume(handle, config, true).is_ok() {
                return Ok(());
            }
        }
        // Passe 2 : n'importe quel volume inscriptible
        for &handle in handles.iter() {
            if try_save_on_volume(handle, config, false).is_ok() {
                return Ok(());
            }
        }
    }

    // Fallback sur le handle par défaut
    let handle = uefi::boot::get_handle_for_protocol::<SimpleFileSystem>()?;
    try_save_on_volume(handle, config, false)
}

#[cfg(not(target_os = "uefi"))]
pub fn save_to_disk(_config: &SpoofConfig) -> Result<(), uefi::Error> {
    Err(uefi::Error::new(uefi::Status::UNSUPPORTED, ()))
}

/// Tente de charger `SpoofConfig` depuis un volume `SimpleFileSystem` donné.
#[cfg(target_os = "uefi")]
fn try_load_from_volume(handle: uefi::Handle) -> Result<SpoofConfig, uefi::Error> {
    let mut fs = uefi::boot::open_protocol_exclusive::<SimpleFileSystem>(handle)?;
    let mut root = fs.open_volume()?;

    let file_handle = root.open(DISK_FILE, FileMode::Read, FileAttribute::empty())?;
    let mut file: RegularFile = file_handle
        .into_regular_file()
        .ok_or_else(|| uefi::Error::new(uefi::Status::DEVICE_ERROR, ()))?;

    let mut config = SpoofConfig::default();
    let data = unsafe {
        core::slice::from_raw_parts_mut(
            (&mut config as *mut SpoofConfig).cast::<u8>(),
            mem::size_of::<SpoofConfig>(),
        )
    };

    let _ = file.read(data)?;

    if config.is_valid() {
        return Ok(config);
    }
    if config.needs_upgrade() {
        config.upgrade_from_older();
        return Ok(config);
    }

    Err(uefi::Error::new(uefi::Status::CRC_ERROR, ()))
}

/// Charge la configuration depuis la partition EFI (`\EFI\SmbiosSpoofer\uuid.dat`)
/// en parcourant tous les volumes `SimpleFileSystem`.
#[cfg(target_os = "uefi")]
pub fn load_from_disk() -> Result<SpoofConfig, uefi::Error> {
    if let Ok(handles) = uefi::boot::locate_handle_buffer(uefi::boot::SearchType::ByProtocol(
        &SimpleFileSystem::GUID,
    )) {
        for &handle in handles.iter() {
            if let Ok(cfg) = try_load_from_volume(handle) {
                return Ok(cfg);
            }
        }
    }

    let handle = uefi::boot::get_handle_for_protocol::<SimpleFileSystem>()?;
    try_load_from_volume(handle)
}

#[cfg(not(target_os = "uefi"))]
pub fn load_from_disk() -> Result<SpoofConfig, uefi::Error> {
    Err(uefi::Error::new(uefi::Status::UNSUPPORTED, ()))
}

/// Essaie de charger : NVRAM d'abord, puis disque ESP en fallback.
/// Si chargé depuis le disque uniquement, resynchronise automatiquement la NVRAM.
pub fn load_config() -> Option<SpoofConfig> {
    if let Ok(cfg) = load_from_nvram() {
        return Some(cfg);
    }
    if let Ok(cfg) = load_from_disk() {
        let _ = save_to_nvram(&cfg);
        return Some(cfg);
    }
    None
}

/// Sauvegarde dans les deux supports (NVRAM + disque ESP) et retourne le statut détaillé.
pub fn save_config(config: &SpoofConfig) -> PersistenceStatus {
    let nvram_ok = save_to_nvram(config).is_ok();
    let disk_ok = save_to_disk(config).is_ok();
    PersistenceStatus { nvram_ok, disk_ok }
}

// ─── Tests unitaires ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spoof_config_generate_and_checksum() {
        let mut cfg = SpoofConfig::default();
        assert!(!cfg.is_valid());

        cfg.generate();
        assert!(cfg.is_valid());
        let magic = cfg.magic;
        let version = cfg.version;
        let ram_seed = cfg.ram_serial_seed;
        let oem_seed = cfg.oem_seed;
        assert_eq!(magic, NVRAM_MAGIC);
        assert_eq!(version, NVRAM_VERSION);
        assert_ne!(ram_seed, 0);
        assert_ne!(oem_seed, 0);
    }

    #[test]
    fn test_spoof_config_upgrade_from_v7() {
        let mut cfg = SpoofConfig::default();
        cfg.generate();
        // Simuler une ancienne config v7 avec nouveaux champs v8 à zéro et ancien checksum
        cfg.version = 7;
        cfg.chassis_asset_tag = [0u16; 64];
        cfg.baseboard_asset_tag = [0u16; 64];
        cfg.system_sku = [0u16; 64];
        cfg.ram_serial_seed = 0;
        cfg.oem_seed = 0;
        cfg.checksum = 0xDEADBEEF;

        assert!(!cfg.is_valid());
        assert!(cfg.needs_upgrade());

        let old_uuid = cfg.uuid;
        let old_sys_serial = cfg.system_serial;
        cfg.upgrade_from_older();

        assert!(cfg.is_valid());
        let version = cfg.version;
        let sys_serial = cfg.system_serial;
        let ram_seed = cfg.ram_serial_seed;
        let chassis_at = cfg.chassis_asset_tag;
        assert_eq!(version, NVRAM_VERSION);
        assert_eq!(cfg.uuid, old_uuid);
        assert_eq!(sys_serial, old_sys_serial);
        assert_ne!(ram_seed, 0);
        assert_ne!(chassis_at[0], 0);
    }
}
