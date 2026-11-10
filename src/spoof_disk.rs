// ─── Spoofing des numéros de série de disques (NVMe / SATA / AHCI / SCSI) ───
//
// Ce module implémente le spoofing non-destructif des identifiants matériels de
// disques au niveau UEFI via `EFI_DISK_INFO_PROTOCOL` (`d432a67f-14dc-484b-b3bb-3f0291849327`).
//
// Sécurité critique (anti-BSOD `0xc000000e`) :
// - Ce module NE MODIFIE JAMAIS les GUID de partitions GPT ni la signature MBR
//   écrits physiquement sur le disque de boot, car le magasin BCD de Windows
//   (`\EFI\Microsoft\Boot\BCD`) référence ces GUID pour localiser `winload.efi`.
// - Toutes les modifications opèrent exclusivement en mémoire RAM (buffers
//   `Identify` NVMe 4096 octets, `IDENTIFY DEVICE` ATA 512 octets swap-word,
//   structures `Inquiry` SCSI, et hooks de fonctions sur `EFI_DISK_INFO_PROTOCOL`).

use crate::persistence::SpoofConfig;
use crate::utils;

#[cfg(target_os = "uefi")]
use core::ffi::c_void;
#[cfg(target_os = "uefi")]
use uefi::Status;
#[cfg(target_os = "uefi")]
use uefi::boot::{self, SearchType};

// ─── Constantes & GUIDs UEFI (`EFI_DISK_INFO_PROTOCOL`) ─────────────────────

/// `EFI_DISK_INFO_PROTOCOL_GUID` = `{d432a67f-14dc-484b-b3bb-3f0291849327}`
pub const DISK_INFO_PROTOCOL_GUID: uefi::Guid = uefi::Guid::from_bytes([
    0x7f, 0xa6, 0x32, 0xd4, 0xdc, 0x14, 0x4b, 0x48,
    0xb3, 0xbb, 0x3f, 0x02, 0x91, 0x84, 0x93, 0x27,
]);

/// `EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL_GUID` = `{52c78312-8edc-4233-98f2-1a1aa5e388a5}`
pub const NVME_PASS_THRU_PROTOCOL_GUID: uefi::Guid = uefi::Guid::from_bytes([
    0x12, 0x83, 0xc7, 0x52, 0xdc, 0x8e, 0x33, 0x42,
    0x98, 0xf2, 0x1a, 0x1a, 0xa5, 0xe3, 0x88, 0xa5,
]);

/// `EFI_DISK_INFO_NVME_INTERFACE_GUID` = `{3ab14680-5d3f-4a4d-bcdc-cc380018c7f7}`
pub const DISK_INFO_NVME_INTERFACE_BYTES: [u8; 16] = [
    0x80, 0x46, 0xb1, 0x3a, 0x3f, 0x5d, 0x4d, 0x4a,
    0xbc, 0xdc, 0xcc, 0x38, 0x00, 0x18, 0xc7, 0xf7,
];

/// `EFI_DISK_INFO_AHCI_INTERFACE_GUID` = `{9e498932-4abc-45af-a34d-0247787be7c6}`
pub const DISK_INFO_AHCI_INTERFACE_BYTES: [u8; 16] = [
    0x32, 0x89, 0x49, 0x9e, 0xbc, 0x4a, 0xaf, 0x45,
    0xa3, 0x4d, 0x02, 0x47, 0x78, 0x7b, 0xe7, 0xc6,
];

/// `EFI_DISK_INFO_IDE_INTERFACE_GUID` = `{5e948fe3-26d3-42b5-af17-610287188dec}`
pub const DISK_INFO_IDE_INTERFACE_BYTES: [u8; 16] = [
    0xe3, 0x8f, 0x94, 0x5e, 0xd3, 0x26, 0xb5, 0x42,
    0xaf, 0x17, 0x61, 0x02, 0x87, 0x18, 0x8d, 0xec,
];

/// Longueur standard d'un numéro de série ATA / NVMe (20 octets ASCII).
pub const DISK_SERIAL_RAW_LEN: usize = 20;
/// Longueur standard d'un nom de modèle ATA / NVMe (40 octets ASCII).
pub const DISK_MODEL_RAW_LEN: usize = 40;

/// Taille minimale d'une structure NVMe `Identify Controller` (4096 octets).
pub const NVME_IDENTIFY_SIZE: usize = 4096;
/// Taille standard d'une structure ATA `IDENTIFY DEVICE` (512 octets / 256 mots).
pub const ATA_IDENTIFY_SIZE: usize = 512;

/// Représentation C du protocole UEFI `EFI_DISK_INFO_PROTOCOL`.
#[cfg(target_os = "uefi")]
#[repr(C)]
#[uefi::proto::unsafe_protocol(DISK_INFO_PROTOCOL_GUID)]
pub struct EfiDiskInfoProtocol {
    pub interface: [u8; 16],
    pub inquiry: unsafe extern "efiapi" fn(
        this: *mut EfiDiskInfoProtocol,
        inquiry_data: *mut c_void,
        inquiry_data_size: *mut u32,
    ) -> Status,
    pub identify: unsafe extern "efiapi" fn(
        this: *mut EfiDiskInfoProtocol,
        identify_data: *mut c_void,
        identify_data_size: *mut u32,
    ) -> Status,
    pub sense_data: unsafe extern "efiapi" fn(
        this: *mut EfiDiskInfoProtocol,
        sense_data: *mut c_void,
        sense_data_size: *mut u32,
        sense_data_number: *mut u8,
    ) -> Status,
    pub which_ide: unsafe extern "efiapi" fn(
        this: *mut EfiDiskInfoProtocol,
        ide_channel: *mut u32,
        ide_device: *mut u32,
    ) -> Status,
}

/// Commande NVMe standard (UEFI Spec `EFI_NVM_EXPRESS_COMMAND`).
#[repr(C)]
pub struct EfiNvmExpressCommand {
    pub cdw0: u32,
    pub flags: u8,
    pub rsvd1: [u8; 3],
    pub nsid: u32,
    pub cdw2: u32,
    pub cdw3: u32,
    pub cdw10: u32,
    pub cdw11: u32,
    pub cdw12: u32,
    pub cdw13: u32,
    pub cdw14: u32,
    pub cdw15: u32,
}

/// Paquet de commande `EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET`.
#[repr(C)]
pub struct EfiNvmExpressPassThruCommandPacket {
    pub command_timeout: u64,
    pub transfer_buffer: *mut core::ffi::c_void,
    pub transfer_length: u32,
    pub metadata_buffer: *mut core::ffi::c_void,
    pub metadata_length: u32,
    pub queue_type: u8,
    pub rsvd: [u8; 7],
    pub nvme_cmd: *mut EfiNvmExpressCommand,
    pub nvme_completion: *mut core::ffi::c_void,
}

/// Protocole UEFI `EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL`.
#[cfg(target_os = "uefi")]
#[repr(C)]
#[uefi::proto::unsafe_protocol(NVME_PASS_THRU_PROTOCOL_GUID)]
pub struct EfiNvmExpressPassThruProtocol {
    pub mode: *mut c_void,
    pub pass_thru: unsafe extern "efiapi" fn(
        this: *mut EfiNvmExpressPassThruProtocol,
        namespace_id: u32,
        packet: *mut EfiNvmExpressPassThruCommandPacket,
        event: *mut c_void,
    ) -> Status,
}

/// Vérifie si une commande NVMe Admin correspond à `Identify Controller` (`Opcode == 0x06`, `CNS == 0x01`).
pub fn is_nvme_admin_identify_controller(queue_type: u8, cdw0: u32, cdw10: u32, transfer_len: u32) -> bool {
    queue_type == 0
        && (cdw0 & 0xFF) == 0x06
        && (cdw10 & 0xFF) == 0x01
        && (transfer_len as usize) >= NVME_IDENTIFY_SIZE
}

// ─── Fonctions pures de parsing & patch (NVMe / ATA / SCSI) ─────────────────

/// Convertit une tranche d'octets ASCII (avec espaces de padding en tête/queue)
/// en buffer UTF-16 terminé par `0`.
fn ascii_trimmed_to_u16(raw: &[u8]) -> [u16; 64] {
    let mut out = [0u16; 64];
    let mut start = 0;
    let mut end = raw.len();

    while start < end && (raw[start] == b' ' || raw[start] == 0) {
        start += 1;
    }
    while end > start && (raw[end - 1] == b' ' || raw[end - 1] == 0) {
        end -= 1;
    }

    let mut idx = 0;
    for &b in &raw[start..end] {
        if idx >= 63 {
            break;
        }
        if (0x20..=0x7E).contains(&b) {
            out[idx] = b as u16;
            idx += 1;
        }
    }
    out[idx] = 0;
    out
}

/// Extrait le numéro de série (`SN`, octets `4..24`, 20 octets ASCII) d'un buffer
/// `NVMe Identify Controller` (4096 octets).
pub fn parse_nvme_identify_serial(data: &[u8]) -> [u16; 64] {
    if data.len() < 24 {
        return [0u16; 64];
    }
    ascii_trimmed_to_u16(&data[4..24])
}

/// Extrait le nom de modèle (`MN`, octets `24..64`, 40 octets ASCII) d'un buffer
/// `NVMe Identify Controller` (4096 octets).
pub fn parse_nvme_identify_model(data: &[u8]) -> [u16; 64] {
    if data.len() < 64 {
        return [0u16; 64];
    }
    ascii_trimmed_to_u16(&data[24..64])
}

/// Patche le numéro de série (`SN`, octets `4..24`) dans un buffer `NVMe Identify Controller`
/// en le complétant avec des espaces ASCII (`0x20`) conformément à la spécification NVMe.
pub fn patch_nvme_identify_serial(data: &mut [u8], new_serial: &[u16]) -> bool {
    if data.len() < 24 {
        return false;
    }
    let len = utils::str_len_u16(new_serial);
    if len == 0 {
        return false;
    }

    let raw = encode_nvme_serial_bytes(new_serial);
    data[4..24].copy_from_slice(&raw);
    true
}

/// Encode un numéro de série UTF-16 en bloc NVMe de 20 octets ASCII (space-padded `0x20`).
pub fn encode_nvme_serial_bytes(serial: &[u16]) -> [u8; DISK_SERIAL_RAW_LEN] {
    let mut raw = [b' '; DISK_SERIAL_RAW_LEN];
    let len = utils::str_len_u16(serial).min(DISK_SERIAL_RAW_LEN);
    for (i, &ch) in serial.iter().take(len).enumerate() {
        let b = (ch & 0xFF) as u8;
        raw[i] = if (0x20..=0x7E).contains(&b) { b } else { b'0' };
    }
    raw
}

/// Décode une tranche d'octets ATA `IDENTIFY DEVICE` où chaque mot de 16 bits
/// a ses deux octets inversés (`[b0, b1]` -> caractères `[b1, b0]`).
fn decode_ata_swapped_bytes(swapped: &[u8], out_ascii: &mut [u8]) {
    let pairs = (swapped.len().min(out_ascii.len())) / 2;
    for i in 0..pairs {
        out_ascii[i * 2] = swapped[i * 2 + 1];
        out_ascii[i * 2 + 1] = swapped[i * 2];
    }
}

/// Encode un numéro de série UTF-16 en bloc ATA de 20 octets avec inversion par paire d'octets
/// (`word-swapped ASCII`, space-padded `0x20`).
pub fn encode_ata_serial_bytes(serial: &[u16]) -> [u8; DISK_SERIAL_RAW_LEN] {
    let plain = encode_nvme_serial_bytes(serial);
    let mut swapped = [b' '; DISK_SERIAL_RAW_LEN];
    for i in 0..(DISK_SERIAL_RAW_LEN / 2) {
        swapped[i * 2] = plain[i * 2 + 1];
        swapped[i * 2 + 1] = plain[i * 2];
    }
    swapped
}

/// Extrait le numéro de série ATA (mots `10..19` = octets `20..40`, 20 octets word-swapped)
/// d'un buffer `ATA IDENTIFY DEVICE` (512 octets).
pub fn parse_ata_identify_serial(data: &[u8]) -> [u16; 64] {
    if data.len() < 40 {
        return [0u16; 64];
    }
    let mut plain = [0u8; DISK_SERIAL_RAW_LEN];
    decode_ata_swapped_bytes(&data[20..40], &mut plain);
    ascii_trimmed_to_u16(&plain)
}

/// Extrait le nom de modèle ATA (mots `27..46` = octets `54..94`, 40 octets word-swapped)
/// d'un buffer `ATA IDENTIFY DEVICE` (512 octets).
pub fn parse_ata_identify_model(data: &[u8]) -> [u16; 64] {
    if data.len() < 94 {
        return [0u16; 64];
    }
    let mut plain = [0u8; DISK_MODEL_RAW_LEN];
    decode_ata_swapped_bytes(&data[54..94], &mut plain);
    ascii_trimmed_to_u16(&plain)
}

/// Patche le numéro de série ATA (mots `10..19` = octets `20..40`) dans un buffer
/// `ATA IDENTIFY DEVICE` (512 octets) en appliquant l'inversion de mots ATA.
pub fn patch_ata_identify_serial(data: &mut [u8], new_serial: &[u16]) -> bool {
    if data.len() < 40 {
        return false;
    }
    let len = utils::str_len_u16(new_serial);
    if len == 0 {
        return false;
    }

    let swapped = encode_ata_serial_bytes(new_serial);
    data[20..40].copy_from_slice(&swapped);
    true
}

/// Extrait le vendeur + modèle d'une réponse `SCSI INQUIRY` standard (octets `8..32`).
pub fn parse_scsi_inquiry_model(data: &[u8]) -> [u16; 64] {
    if data.len() < 32 {
        return [0u16; 64];
    }
    ascii_trimmed_to_u16(&data[8..32])
}

/// Patche le numéro de série dans une réponse `SCSI INQUIRY` étendue (octets `36..56`)
/// lorsque le périphérique expose un champ Vendor-Specific Serial dans son Inquiry.
pub fn patch_scsi_inquiry_serial(data: &mut [u8], new_serial: &[u16]) -> bool {
    if data.len() < 56 {
        return false;
    }
    let len = utils::str_len_u16(new_serial);
    if len == 0 {
        return false;
    }
    let raw = encode_nvme_serial_bytes(new_serial);
    data[36..56].copy_from_slice(&raw);
    true
}

// ─── Hooks et exécution UEFI (`EFI_DISK_INFO_PROTOCOL`) ─────────────────────

#[cfg(target_os = "uefi")]
const MAX_HOOKED_DISKS: usize = 16;

#[cfg(target_os = "uefi")]
#[derive(Clone, Copy)]
struct HookedDiskSlot {
    active: bool,
    is_nvme: bool,
    protocol_ptr: usize,
    orig_identify: Option<
        unsafe extern "efiapi" fn(
            this: *mut EfiDiskInfoProtocol,
            identify_data: *mut c_void,
            identify_data_size: *mut u32,
        ) -> Status,
    >,
    orig_inquiry: Option<
        unsafe extern "efiapi" fn(
            this: *mut EfiDiskInfoProtocol,
            inquiry_data: *mut c_void,
            inquiry_data_size: *mut u32,
        ) -> Status,
    >,
    orig_serial: [u16; 64],
    orig_model: [u16; 64],
    spoof_serial: [u16; 64],
}

#[cfg(target_os = "uefi")]
impl HookedDiskSlot {
    const fn empty() -> Self {
        Self {
            active: false,
            is_nvme: false,
            protocol_ptr: 0,
            orig_identify: None,
            orig_inquiry: None,
            orig_serial: [0u16; 64],
            orig_model: [0u16; 64],
            spoof_serial: [0u16; 64],
        }
    }
}

#[cfg(target_os = "uefi")]
static mut HOOKED_DISKS: [HookedDiskSlot; MAX_HOOKED_DISKS] =
    [HookedDiskSlot::empty(); MAX_HOOKED_DISKS];

#[cfg(target_os = "uefi")]
static mut CACHED_ORIG_FIRST_SERIAL: [u16; 64] = [0u16; 64];
#[cfg(target_os = "uefi")]
static mut CACHED_ORIG_FIRST_MODEL: [u16; 64] = [0u16; 64];
#[cfg(target_os = "uefi")]
static mut CACHED_ORIG_DISK_COUNT: usize = 0;

#[cfg(target_os = "uefi")]
const MAX_HOOKED_NVME_PASS_THRU: usize = 8;

#[cfg(target_os = "uefi")]
#[derive(Clone, Copy)]
struct HookedNvmePassThruSlot {
    active: bool,
    protocol_ptr: usize,
    orig_pass_thru: Option<
        unsafe extern "efiapi" fn(
            this: *mut EfiNvmExpressPassThruProtocol,
            namespace_id: u32,
            packet: *mut EfiNvmExpressPassThruCommandPacket,
            event: *mut c_void,
        ) -> Status,
    >,
    spoof_serial: [u16; 64],
}

#[cfg(target_os = "uefi")]
impl HookedNvmePassThruSlot {
    const fn empty() -> Self {
        Self {
            active: false,
            protocol_ptr: 0,
            orig_pass_thru: None,
            spoof_serial: [0u16; 64],
        }
    }
}

#[cfg(target_os = "uefi")]
static mut HOOKED_NVME_PASS_THRU: [HookedNvmePassThruSlot; MAX_HOOKED_NVME_PASS_THRU] =
    [HookedNvmePassThruSlot::empty(); MAX_HOOKED_NVME_PASS_THRU];

/// Hook UEFI sur `EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL.PassThru`.
/// Intercepte les commandes Admin `Identify Controller` (`Opcode 0x06`, `CNS 0x01`)
/// et remplace le numéro de série NVMe (`octets 4..24`) dans `TransferBuffer`.
#[cfg(target_os = "uefi")]
unsafe extern "efiapi" fn hooked_nvme_pass_thru(
    this: *mut EfiNvmExpressPassThruProtocol,
    namespace_id: u32,
    packet: *mut EfiNvmExpressPassThruCommandPacket,
    event: *mut c_void,
) -> Status {
    let ptr_val = this as usize;
    let mut slot_opt: Option<HookedNvmePassThruSlot> = None;

    unsafe {
        let slots_ptr = core::ptr::addr_of!(HOOKED_NVME_PASS_THRU);
        for i in 0..MAX_HOOKED_NVME_PASS_THRU {
            let slot = (*slots_ptr)[i];
            if slot.active && slot.protocol_ptr == ptr_val {
                slot_opt = Some(slot);
                break;
            }
        }
    }

    let Some(slot) = slot_opt else {
        return Status::DEVICE_ERROR;
    };
    let Some(orig_fn) = slot.orig_pass_thru else {
        return Status::UNSUPPORTED;
    };

    let status = unsafe { orig_fn(this, namespace_id, packet, event) };
    if status == Status::SUCCESS && !packet.is_null() {
        unsafe {
            let pkt = &*packet;
            if !pkt.nvme_cmd.is_null() && !pkt.transfer_buffer.is_null() {
                let cmd = &*pkt.nvme_cmd;
                if is_nvme_admin_identify_controller(
                    pkt.queue_type,
                    cmd.cdw0,
                    cmd.cdw10,
                    pkt.transfer_length,
                ) {
                    let slice = core::slice::from_raw_parts_mut(
                        pkt.transfer_buffer as *mut u8,
                        pkt.transfer_length as usize,
                    );
                    let _ = patch_nvme_identify_serial(slice, &slot.spoof_serial);
                }
            }
        }
    }
    status
}

/// Hook UEFI sur `EFI_DISK_INFO_PROTOCOL.Identify`.
/// Appelle la routine `Identify` originale du pilote DXE (NVMe / AHCI / SATA),
/// puis remplace à la volée le numéro de série dans le buffer retourné.
#[cfg(target_os = "uefi")]
unsafe extern "efiapi" fn hooked_disk_identify(
    this: *mut EfiDiskInfoProtocol,
    identify_data: *mut c_void,
    identify_data_size: *mut u32,
) -> Status {
    let ptr_val = this as usize;
    let mut slot_opt: Option<HookedDiskSlot> = None;

    unsafe {
        let slots_ptr = core::ptr::addr_of!(HOOKED_DISKS);
        for i in 0..MAX_HOOKED_DISKS {
            let slot = (*slots_ptr)[i];
            if slot.active && slot.protocol_ptr == ptr_val {
                slot_opt = Some(slot);
                break;
            }
        }
    }

    let Some(slot) = slot_opt else {
        return Status::DEVICE_ERROR;
    };
    let Some(orig_fn) = slot.orig_identify else {
        return Status::UNSUPPORTED;
    };

    let status = unsafe { orig_fn(this, identify_data, identify_data_size) };
    if status == Status::SUCCESS && !identify_data.is_null() && !identify_data_size.is_null() {
        let size = unsafe { *identify_data_size as usize };
        if size >= 40 {
            let slice = unsafe { core::slice::from_raw_parts_mut(identify_data as *mut u8, size) };
            if slot.is_nvme || size >= NVME_IDENTIFY_SIZE {
                let _ = patch_nvme_identify_serial(slice, &slot.spoof_serial);
            } else {
                let _ = patch_ata_identify_serial(slice, &slot.spoof_serial);
            }
        }
    }
    status
}

/// Hook UEFI sur `EFI_DISK_INFO_PROTOCOL.Inquiry`.
/// Appelle la routine `Inquiry` originale du pilote DXE, puis remplace le numéro de série
/// si le buffer d'Inquiry étendu (>= 56 octets) contient le champ série.
#[cfg(target_os = "uefi")]
unsafe extern "efiapi" fn hooked_disk_inquiry(
    this: *mut EfiDiskInfoProtocol,
    inquiry_data: *mut c_void,
    inquiry_data_size: *mut u32,
) -> Status {
    let ptr_val = this as usize;
    let mut slot_opt: Option<HookedDiskSlot> = None;

    unsafe {
        let slots_ptr = core::ptr::addr_of!(HOOKED_DISKS);
        for i in 0..MAX_HOOKED_DISKS {
            let slot = (*slots_ptr)[i];
            if slot.active && slot.protocol_ptr == ptr_val {
                slot_opt = Some(slot);
                break;
            }
        }
    }

    let Some(slot) = slot_opt else {
        return Status::DEVICE_ERROR;
    };
    let Some(orig_fn) = slot.orig_inquiry else {
        return Status::UNSUPPORTED;
    };

    let status = unsafe { orig_fn(this, inquiry_data, inquiry_data_size) };
    if status == Status::SUCCESS && !inquiry_data.is_null() && !inquiry_data_size.is_null() {
        let size = unsafe { *inquiry_data_size as usize };
        if size >= 56 {
            let slice = unsafe { core::slice::from_raw_parts_mut(inquiry_data as *mut u8, size) };
            let _ = patch_scsi_inquiry_serial(slice, &slot.spoof_serial);
        }
    }
    status
}

/// Détermine si un protocole `EFI_DISK_INFO_PROTOCOL` correspond à un contrôleur NVMe.
#[cfg(target_os = "uefi")]
fn is_nvme_interface(interface_guid: &[u8; 16], identify_size: usize) -> bool {
    *interface_guid == DISK_INFO_NVME_INTERFACE_BYTES || identify_size >= NVME_IDENTIFY_SIZE
}

/// Recherche ou alloue un slot dans `HOOKED_DISKS` pour l'instance `protocol_ptr`.
#[cfg(target_os = "uefi")]
unsafe fn find_or_alloc_slot(protocol_ptr: usize) -> Option<usize> {
    unsafe {
        let slots_ptr = core::ptr::addr_of_mut!(HOOKED_DISKS);
        for i in 0..MAX_HOOKED_DISKS {
            if (*slots_ptr)[i].active && (*slots_ptr)[i].protocol_ptr == protocol_ptr {
                return Some(i);
            }
        }
        for i in 0..MAX_HOOKED_DISKS {
            if !(*slots_ptr)[i].active {
                return Some(i);
            }
        }
    }
    None
}

/// Lit les informations matérielles d'un disque via son `EFI_DISK_INFO_PROTOCOL`
/// (en utilisant le pointeur `orig_identify` sauvegardé si le disque est déjà hooké).
#[cfg(target_os = "uefi")]
unsafe fn query_disk_hardware(
    disk_info: *mut EfiDiskInfoProtocol,
) -> Option<(bool, [u16; 64], [u16; 64])> {
    if disk_info.is_null() {
        return None;
    }

    let ptr_val = disk_info as usize;
    let mut orig_identify_fn = unsafe { (*disk_info).identify };
    let mut orig_inquiry_fn = unsafe { (*disk_info).inquiry };

    unsafe {
        let slots_ptr = core::ptr::addr_of!(HOOKED_DISKS);
        for i in 0..MAX_HOOKED_DISKS {
            let slot = (*slots_ptr)[i];
            if slot.active && slot.protocol_ptr == ptr_val {
                if let Some(f) = slot.orig_identify {
                    orig_identify_fn = f;
                }
                if let Some(f) = slot.orig_inquiry {
                    orig_inquiry_fn = f;
                }
                if utils::str_len_u16(&slot.orig_serial) > 0 {
                    return Some((slot.is_nvme, slot.orig_serial, slot.orig_model));
                }
                break;
            }
        }
    }

    let iface = unsafe { (*disk_info).interface };
    let mut buf = [0u8; NVME_IDENTIFY_SIZE];
    let mut size = NVME_IDENTIFY_SIZE as u32;

    let status = unsafe {
        orig_identify_fn(
            disk_info,
            buf.as_mut_ptr() as *mut c_void,
            &mut size as *mut u32,
        )
    };

    if status == Status::SUCCESS && (size as usize) >= 40 {
        let is_nvme = is_nvme_interface(&iface, size as usize);
        let (serial, model) = if is_nvme {
            (
                parse_nvme_identify_serial(&buf[..size as usize]),
                parse_nvme_identify_model(&buf[..size as usize]),
            )
        } else {
            (
                parse_ata_identify_serial(&buf[..size as usize]),
                parse_ata_identify_model(&buf[..size as usize]),
            )
        };
        if utils::str_len_u16(&serial) > 0 || utils::str_len_u16(&model) > 0 {
            return Some((is_nvme, serial, model));
        }
    }

    // Fallback : essai avec exactement 512 octets (certains pilotes AHCI exigent 512)
    let mut ata_buf = [0u8; ATA_IDENTIFY_SIZE];
    let mut ata_size = ATA_IDENTIFY_SIZE as u32;
    let status_ata = unsafe {
        orig_identify_fn(
            disk_info,
            ata_buf.as_mut_ptr() as *mut c_void,
            &mut ata_size as *mut u32,
        )
    };
    if status_ata == Status::SUCCESS && (ata_size as usize) >= 40 {
        let serial = parse_ata_identify_serial(&ata_buf[..ata_size as usize]);
        let model = parse_ata_identify_model(&ata_buf[..ata_size as usize]);
        if utils::str_len_u16(&serial) > 0 || utils::str_len_u16(&model) > 0 {
            return Some((false, serial, model));
        }
    }

    // Fallback SCSI / USB Inquiry
    let mut inq_buf = [0u8; 96];
    let mut inq_size = 96u32;
    let status_inq = unsafe {
        orig_inquiry_fn(
            disk_info,
            inq_buf.as_mut_ptr() as *mut c_void,
            &mut inq_size as *mut u32,
        )
    };
    if status_inq == Status::SUCCESS && (inq_size as usize) >= 32 {
        let model = parse_scsi_inquiry_model(&inq_buf[..inq_size as usize]);
        let serial = if (inq_size as usize) >= 56 {
            ascii_trimmed_to_u16(&inq_buf[36..56])
        } else {
            [0u16; 64]
        };
        if utils::str_len_u16(&model) > 0 || utils::str_len_u16(&serial) > 0 {
            return Some((false, serial, model));
        }
    }

    None
}

/// Patche en mémoire le buffer `Identify` éventuellement mis en cache dans la structure
/// privée du pilote DXE autour de `disk_info_ptr` (sans franchir de frontière de page non allouée).
#[cfg(target_os = "uefi")]
unsafe fn patch_cached_driver_context(
    disk_info_ptr: *mut EfiDiskInfoProtocol,
    is_nvme: bool,
    orig_serial: &[u16; 64],
    new_serial: &[u16; 64],
) {
    if utils::str_len_u16(orig_serial) < 4 || utils::str_len_u16(new_serial) == 0 {
        return;
    }

    let needle = if is_nvme {
        encode_nvme_serial_bytes(orig_serial)
    } else {
        encode_ata_serial_bytes(orig_serial)
    };
    let replacement = if is_nvme {
        encode_nvme_serial_bytes(new_serial)
    } else {
        encode_ata_serial_bytes(new_serial)
    };

    if needle == replacement {
        return;
    }

    // Reste strictement à l'intérieur de la même page 4 Ko que `disk_info_ptr`
    // pour éviter tout Page Fault sur une page adjacente non mappée.
    let base_addr = (disk_info_ptr as usize) & !0xFFFusize;
    let page_ptr = base_addr as *mut u8;
    let page_len = 4096usize;

    let slice = unsafe { core::slice::from_raw_parts_mut(page_ptr, page_len) };
    let mut offset = 0usize;
    while offset + DISK_SERIAL_RAW_LEN <= page_len {
        if slice[offset..offset + DISK_SERIAL_RAW_LEN] == needle {
            slice[offset..offset + DISK_SERIAL_RAW_LEN].copy_from_slice(&replacement);
            offset += DISK_SERIAL_RAW_LEN;
        } else {
            offset += 1;
        }
    }
}

/// Capture le nombre de disques physiques et le numéro de série / modèle d'origine
/// du premier disque détecté via `EFI_DISK_INFO_PROTOCOL` avant tout patch.
#[cfg(target_os = "uefi")]
pub fn read_original_disks(
    first_serial: &mut [u16; 64],
    first_model: &mut [u16; 64],
) -> usize {
    unsafe {
        let cached_count = *core::ptr::addr_of!(CACHED_ORIG_DISK_COUNT);
        if cached_count > 0 {
            let s = &*core::ptr::addr_of!(CACHED_ORIG_FIRST_SERIAL);
            let m = &*core::ptr::addr_of!(CACHED_ORIG_FIRST_MODEL);
            first_serial.copy_from_slice(s);
            first_model.copy_from_slice(m);
            return cached_count;
        }
    }

    first_serial.fill(0);
    first_model.fill(0);

    let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&DISK_INFO_PROTOCOL_GUID))
    else {
        return 0;
    };

    let mut disk_count = 0usize;
    for handle in handles.iter() {
        let Ok(disk_proto) = (unsafe {
            boot::open_protocol::<EfiDiskInfoProtocol>(
                boot::OpenProtocolParams {
                    handle: *handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                boot::OpenProtocolAttributes::GetProtocol,
            )
        }) else {
            continue;
        };

        let disk_info_ptr = &*disk_proto as *const EfiDiskInfoProtocol as *mut EfiDiskInfoProtocol;
        if let Some((_is_nvme, serial, model)) = unsafe { query_disk_hardware(disk_info_ptr) } {
            if disk_count == 0 {
                first_serial.copy_from_slice(&serial);
                first_model.copy_from_slice(&model);
            }
            disk_count += 1;
        }
    }

    if disk_count > 0 {
        unsafe {
            *core::ptr::addr_of_mut!(CACHED_ORIG_DISK_COUNT) = disk_count;
            (*core::ptr::addr_of_mut!(CACHED_ORIG_FIRST_SERIAL)).copy_from_slice(first_serial);
            (*core::ptr::addr_of_mut!(CACHED_ORIG_FIRST_MODEL)).copy_from_slice(first_model);
        }
    }

    disk_count
}

#[cfg(not(target_os = "uefi"))]
pub fn read_original_disks(
    first_serial: &mut [u16; 64],
    first_model: &mut [u16; 64],
) -> usize {
    first_serial.fill(0);
    first_model.fill(0);
    0
}

/// Spoofe tous les disques physiques exposant `EFI_DISK_INFO_PROTOCOL` en mémoire RAM.
/// Retourne le nombre de disques patchés avec succès.
#[cfg(target_os = "uefi")]
pub fn spoof_disks(config: &SpoofConfig) -> u32 {
    let Ok(handles) = boot::locate_handle_buffer(SearchType::ByProtocol(&DISK_INFO_PROTOCOL_GUID))
    else {
        return 0;
    };

    let mut spoofed_count: u32 = 0;

    for (idx, handle) in handles.iter().enumerate() {
        let Ok(mut disk_proto) = (unsafe {
            boot::open_protocol::<EfiDiskInfoProtocol>(
                boot::OpenProtocolParams {
                    handle: *handle,
                    agent: boot::image_handle(),
                    controller: None,
                },
                boot::OpenProtocolAttributes::GetProtocol,
            )
        }) else {
            continue;
        };

        let disk_info_ptr = &mut *disk_proto as *mut EfiDiskInfoProtocol;
        let Some((is_nvme, orig_serial, orig_model)) =
            (unsafe { query_disk_hardware(disk_info_ptr) })
        else {
            continue;
        };

        let Some(slot_idx) = (unsafe { find_or_alloc_slot(disk_info_ptr as usize) }) else {
            continue;
        };

        let cfg_disk_serial = config.disk_serial;
        let cfg_disk_seed = config.disk_serial_seed;
        let mut target_serial = [0u16; 64];
        if idx == 0 && utils::str_len_u16(&cfg_disk_serial) > 0 {
            target_serial.copy_from_slice(&cfg_disk_serial);
        } else {
            utils::generate_disk_serial_deterministic(
                &mut target_serial,
                &orig_serial,
                cfg_disk_seed,
                idx as u32,
            );
        }

        unsafe {
            let slots_ptr = core::ptr::addr_of_mut!(HOOKED_DISKS);
            let slot = &mut (*slots_ptr)[slot_idx];
            if !slot.active {
                slot.active = true;
                slot.is_nvme = is_nvme;
                slot.protocol_ptr = disk_info_ptr as usize;
                slot.orig_identify = Some((*disk_info_ptr).identify);
                slot.orig_inquiry = Some((*disk_info_ptr).inquiry);
                slot.orig_serial = orig_serial;
                slot.orig_model = orig_model;
            }
            slot.spoof_serial = target_serial;

            // Patche le cache interne éventuel dans la page du pilote DXE
            patch_cached_driver_context(disk_info_ptr, is_nvme, &orig_serial, &target_serial);

            // Installe les hooks sur la table de protocole en RAM
            (*disk_info_ptr).identify = hooked_disk_identify;
            (*disk_info_ptr).inquiry = hooked_disk_inquiry;
        }

        spoofed_count += 1;
    }

    // Hook également tous les contrôleurs `EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL`
    if let Ok(nvme_handles) =
        boot::locate_handle_buffer(SearchType::ByProtocol(&NVME_PASS_THRU_PROTOCOL_GUID))
    {
        let cfg_disk_serial = config.disk_serial;
        let cfg_disk_seed = config.disk_serial_seed;
        for (idx, handle) in nvme_handles.iter().take(MAX_HOOKED_NVME_PASS_THRU).enumerate() {
            let Ok(mut nvme_proto) = (unsafe {
                boot::open_protocol::<EfiNvmExpressPassThruProtocol>(
                    boot::OpenProtocolParams {
                        handle: *handle,
                        agent: boot::image_handle(),
                        controller: None,
                    },
                    boot::OpenProtocolAttributes::GetProtocol,
                )
            }) else {
                continue;
            };

            let nvme_ptr = &mut *nvme_proto as *mut EfiNvmExpressPassThruProtocol;
            let mut target_serial = [0u16; 64];
            if idx == 0 && utils::str_len_u16(&cfg_disk_serial) > 0 {
                target_serial.copy_from_slice(&cfg_disk_serial);
            } else {
                utils::generate_disk_serial_deterministic(
                    &mut target_serial,
                    &cfg_disk_serial,
                    cfg_disk_seed,
                    idx as u32,
                );
            }

            unsafe {
                let slots_ptr = core::ptr::addr_of_mut!(HOOKED_NVME_PASS_THRU);
                let mut target_slot: Option<usize> = None;
                for i in 0..MAX_HOOKED_NVME_PASS_THRU {
                    if (*slots_ptr)[i].active && (*slots_ptr)[i].protocol_ptr == nvme_ptr as usize {
                        target_slot = Some(i);
                        break;
                    }
                }
                if target_slot.is_none() {
                    for i in 0..MAX_HOOKED_NVME_PASS_THRU {
                        if !(*slots_ptr)[i].active {
                            target_slot = Some(i);
                            break;
                        }
                    }
                }
                if let Some(s_idx) = target_slot {
                    let slot = &mut (*slots_ptr)[s_idx];
                    if !slot.active {
                        slot.active = true;
                        slot.protocol_ptr = nvme_ptr as usize;
                        slot.orig_pass_thru = Some((*nvme_ptr).pass_thru);
                    }
                    slot.spoof_serial = target_serial;
                    (*nvme_ptr).pass_thru = hooked_nvme_pass_thru;
                }
            }
        }
    }

    spoofed_count
}

#[cfg(not(target_os = "uefi"))]
pub fn spoof_disks(_config: &SpoofConfig) -> u32 {
    0
}

// ─── Tests unitaires (NVMe & ATA Identify parsing/patching) ─────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn str_to_u16_buf(s: &str) -> [u16; 64] {
        let mut buf = [0u16; 64];
        for (i, b) in s.bytes().take(63).enumerate() {
            buf[i] = b as u16;
        }
        buf
    }

    fn u16_buf_to_string(buf: &[u16; 64]) -> String {
        let len = utils::str_len_u16(buf);
        buf[..len]
            .iter()
            .map(|&c| (c & 0xFF) as u8 as char)
            .collect()
    }

    #[test]
    fn test_nvme_identify_parse_and_patch() {
        let mut nvme_data = vec![0u8; NVME_IDENTIFY_SIZE];
        // Bytes 4..24 = Serial Number (20 bytes, space-padded)
        nvme_data[4..24].copy_from_slice(b"S4EVNF0M812345X     ");
        // Bytes 24..64 = Model Number (40 bytes, space-padded)
        nvme_data[24..64].copy_from_slice(b"Samsung SSD 970 EVO Plus 1TB            ");

        let parsed_sn = parse_nvme_identify_serial(&nvme_data);
        let parsed_mn = parse_nvme_identify_model(&nvme_data);
        assert_eq!(u16_buf_to_string(&parsed_sn), "S4EVNF0M812345X");
        assert_eq!(u16_buf_to_string(&parsed_mn), "Samsung SSD 970 EVO Plus 1TB");

        let new_sn = str_to_u16_buf("K9TRPL4B990123Z");
        assert!(patch_nvme_identify_serial(&mut nvme_data, &new_sn));

        let reparsed_sn = parse_nvme_identify_serial(&nvme_data);
        assert_eq!(u16_buf_to_string(&reparsed_sn), "K9TRPL4B990123Z");
        // Verify space-padding in raw bytes 4..24
        assert_eq!(&nvme_data[4..24], b"K9TRPL4B990123Z     ");
        // Model number untouched
        let reparsed_mn = parse_nvme_identify_model(&nvme_data);
        assert_eq!(u16_buf_to_string(&reparsed_mn), "Samsung SSD 970 EVO Plus 1TB");
    }

    #[test]
    fn test_ata_identify_word_swapped_parse_and_patch() {
        let mut ata_data = vec![0u8; ATA_IDENTIFY_SIZE];
        // Simulate ATA word-swapped serial "WD-WCC6Y7ABC123"
        let orig_sn = str_to_u16_buf("WD-WCC6Y7ABC123");
        let swapped_sn = encode_ata_serial_bytes(&orig_sn);
        ata_data[20..40].copy_from_slice(&swapped_sn);

        // Verify that raw bytes are indeed byte-swapped pairs ("DW-W..." instead of "WD-W...")
        assert_eq!(ata_data[20], b'D');
        assert_eq!(ata_data[21], b'W');

        let parsed_sn = parse_ata_identify_serial(&ata_data);
        assert_eq!(u16_buf_to_string(&parsed_sn), "WD-WCC6Y7ABC123");

        let new_sn = str_to_u16_buf("WD-XRT9M2KLP884");
        assert!(patch_ata_identify_serial(&mut ata_data, &new_sn));

        let reparsed_sn = parse_ata_identify_serial(&ata_data);
        assert_eq!(u16_buf_to_string(&reparsed_sn), "WD-XRT9M2KLP884");
    }

    #[test]
    fn test_scsi_inquiry_parse_and_patch() {
        let mut inq = vec![0u8; 64];
        inq[8..32].copy_from_slice(b"KINGSTONSA400S37480G    ");
        inq[36..56].copy_from_slice(b"50026B768211ABCD    ");

        let model = parse_scsi_inquiry_model(&inq);
        assert_eq!(u16_buf_to_string(&model), "KINGSTONSA400S37480G");

        let new_sn = str_to_u16_buf("50026B769988DCBA");
        assert!(patch_scsi_inquiry_serial(&mut inq, &new_sn));
        assert_eq!(&inq[36..56], b"50026B769988DCBA    ");
    }

    #[test]
    fn test_nvme_admin_identify_controller_detection() {
        // Admin queue (0), Opcode 0x06 (Identify), CNS 0x01 (Controller), 4096 bytes
        assert!(is_nvme_admin_identify_controller(0, 0x0000_0006, 0x0000_0001, 4096));
        // Non-admin queue (1) -> false
        assert!(!is_nvme_admin_identify_controller(1, 0x0000_0006, 0x0000_0001, 4096));
        // Namespace identify (CNS 0x00) -> false
        assert!(!is_nvme_admin_identify_controller(0, 0x0000_0006, 0x0000_0000, 4096));
    }
}
