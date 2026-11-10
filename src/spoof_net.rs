//! Module de spoofing d'adresse MAC via UEFI Simple Network Protocol (SNP) + NVRAM.
//!
//! ## Stratégies combinées
//!
//! 1. **Capture de la MAC matérielle originale (`read_original_mac`)** :
//!    Sauvegarde l'adresse MAC d'usine (`permanent_address` / `current_address`) en cache
//!    statique avant tout patch pour l'affichage `orig -> new`.
//! 2. **SNP `StationAddress()` + Patch direct de `SimpleNetworkMode` en RAM** :
//!    - Appelle `snp.station_address(false, Some(&efi_mac))` sans éteindre l'interface (`shutdown()`
//!      réinitialiserait l'adresse sur de nombreux firmwares UNDI).
//!    - Écrase également en place `mode.current_address` et `mode.permanent_address` dans la
//!      structure `SimpleNetworkMode` en mémoire DXE.
//! 3. **Nettoyage / remplacement des variables NVRAM MAC connues** (`NetworkMacAddress`, `MacAddress`, etc.).

use crate::persistence::SpoofConfig;

#[cfg(target_os = "uefi")]
use uefi::Identify;
#[cfg(target_os = "uefi")]
use uefi::proto::network::EfiMacAddr;
#[cfg(target_os = "uefi")]
use uefi::proto::network::snp::{NetworkMode, NetworkState, SimpleNetwork};

#[cfg(target_os = "uefi")]
static mut CACHED_ORIG_MAC: [u8; 6] = [0u8; 6];
#[cfg(target_os = "uefi")]
static mut CACHED_ORIG_MAC_VALID: bool = false;

/// Lit l'adresse MAC matérielle originale de la première carte réseau UEFI (`SimpleNetwork`)
/// avant tout spoofing et la met en cache pour les régénérations `[R]`.
#[cfg(target_os = "uefi")]
pub fn read_original_mac(out_mac: &mut [u8; 6]) -> bool {
    unsafe {
        if *core::ptr::addr_of!(CACHED_ORIG_MAC_VALID) {
            let cached = &*core::ptr::addr_of!(CACHED_ORIG_MAC);
            out_mac.copy_from_slice(cached);
            return true;
        }
    }

    let Ok(handles) = uefi::boot::locate_handle_buffer(uefi::boot::SearchType::ByProtocol(
        &SimpleNetwork::GUID,
    )) else {
        return false;
    };

    for &handle in handles.iter() {
        let Ok(snp) = uefi::boot::open_protocol_exclusive::<SimpleNetwork>(handle) else {
            continue;
        };
        let mode = snp.mode();
        let perm = &mode.permanent_address.0[..6];
        let curr = &mode.current_address.0[..6];

        let chosen = if is_likely_mac_address(perm) {
            Some(perm)
        } else if is_likely_mac_address(curr) {
            Some(curr)
        } else {
            None
        };

        if let Some(mac) = chosen {
            out_mac.copy_from_slice(mac);
            unsafe {
                (*core::ptr::addr_of_mut!(CACHED_ORIG_MAC)).copy_from_slice(mac);
                *core::ptr::addr_of_mut!(CACHED_ORIG_MAC_VALID) = true;
            }
            return true;
        }
    }

    false
}

#[cfg(not(target_os = "uefi"))]
pub fn read_original_mac(_out_mac: &mut [u8; 6]) -> bool {
    false
}

/// Spoofe l'adresse MAC via le protocole UEFI `SimpleNetwork` (SNP) et patche la structure
/// `SimpleNetworkMode` (`current_address` et `permanent_address`) directement en RAM.
///
/// # Safety
/// Manipule les pointeurs de protocole SNP en mémoire DXE.
#[cfg(target_os = "uefi")]
pub unsafe fn spoof_mac_via_snp(config: &SpoofConfig) -> u32 {
    let mac_local = config.mac_address;
    if !is_likely_mac_address(&mac_local) {
        return 0;
    }

    let Ok(handles) = uefi::boot::locate_handle_buffer(uefi::boot::SearchType::ByProtocol(
        &SimpleNetwork::GUID,
    )) else {
        return 0;
    };

    let mut count: u32 = 0;

    for (idx, &handle) in handles.iter().enumerate() {
        let Ok(snp) = uefi::boot::open_protocol_exclusive::<SimpleNetwork>(handle) else {
            continue;
        };

        let mode = snp.mode();
        if mode.state == NetworkState::STOPPED {
            let _ = snp.start();
        }
        if snp.mode().state == NetworkState::STARTED {
            let _ = snp.initialize(0, 0);
        }

        // Dériver une MAC unique par interface si la machine possède plusieurs NICs
        let mut nic_mac = mac_local;
        if idx > 0 {
            nic_mac[5] = nic_mac[5].wrapping_add(idx as u8);
        }

        let mut efi_mac = EfiMacAddr([0u8; 32]);
        efi_mac.0[0..6].copy_from_slice(&nic_mac);

        let station_ok = snp.station_address(false, Some(&efi_mac)).is_ok();

        // Patcher également `current_address` et `permanent_address` dans `NetworkMode` en RAM
        // (et ne PAS appeler `snp.shutdown()` pour conserver l'état programmé dans le contrôleur UNDI).
        let mode_ptr = snp.mode() as *const NetworkMode as *mut NetworkMode;
        if !mode_ptr.is_null() {
            unsafe {
                (*mode_ptr).current_address = efi_mac;
                (*mode_ptr).permanent_address = efi_mac;
            }
        }

        if station_ok || !mode_ptr.is_null() {
            count += 1;
        }
    }

    count
}

#[cfg(not(target_os = "uefi"))]
pub unsafe fn spoof_mac_via_snp(_config: &SpoofConfig) -> u32 {
    0
}

/// Noms de variables NVRAM connues contenant des adresses MAC ou tags matériels.
#[cfg(target_os = "uefi")]
const VAR_NET_MAC: &uefi::CStr16 = uefi::cstr16!("NetworkMacAddress");
#[cfg(target_os = "uefi")]
const VAR_MAC_ADDR: &uefi::CStr16 = uefi::cstr16!("MacAddress");
#[cfg(target_os = "uefi")]
const VAR_SYS_MAC: &uefi::CStr16 = uefi::cstr16!("SystemMacAddress");
#[cfg(target_os = "uefi")]
const VAR_ASSET_TAG: &uefi::CStr16 = uefi::cstr16!("AssetTag");

/// GUID pour les variables réseau globales EDK2.
#[cfg(target_os = "uefi")]
const NETWORK_GUID: uefi::Guid = uefi::Guid::new(
    [0xD3, 0x90, 0x5A, 0x43],
    [0xC3, 0xCD],
    [0x49, 0x48],
    0x8E,
    0x31,
    [0x3C, 0x28, 0xCA, 0x4F, 0xB1, 0x6B],
);

/// GUID vendor Dell pour AssetTag / MAC.
#[cfg(target_os = "uefi")]
const DELL_GUID: uefi::Guid = uefi::Guid::new(
    [0x16, 0x84, 0x1B, 0xDE],
    [0x5D, 0x89],
    [0x43, 0x4D],
    0x92,
    0x79,
    [0x8A, 0xA2, 0x2F, 0xEE, 0x48, 0x46],
);

/// Cherche et écrase les variables NVRAM contenant potentiellement des adresses MAC.
///
/// # Safety
/// Appelle les UEFI Runtime Services (`GetVariable` / `SetVariable`).
#[cfg(target_os = "uefi")]
pub unsafe fn spoof_nvram_mac_addresses(config: &SpoofConfig) -> u32 {
    let mac_local = config.mac_address;
    if !is_likely_mac_address(&mac_local) {
        return 0;
    }

    let mut count: u32 = 0;
    let vendor_nvram = uefi::runtime::VariableVendor(NETWORK_GUID);
    let vendor_dell = uefi::runtime::VariableVendor(DELL_GUID);
    let vendor_global = uefi::runtime::VariableVendor::GLOBAL_VARIABLE;

    let candidates: &[(&uefi::CStr16, &uefi::runtime::VariableVendor)] = &[
        (VAR_NET_MAC, &vendor_nvram),
        (VAR_MAC_ADDR, &vendor_nvram),
        (VAR_SYS_MAC, &vendor_global),
        (VAR_ASSET_TAG, &vendor_dell),
    ];

    let attrs = uefi::runtime::VariableAttributes::BOOTSERVICE_ACCESS
        | uefi::runtime::VariableAttributes::NON_VOLATILE;

    for &(var_name, vendor) in candidates {
        let mut data_buf = [0u8; 64];
        if uefi::runtime::get_variable(var_name, vendor, &mut data_buf).is_ok()
            && is_likely_mac_address(&data_buf)
            && uefi::runtime::set_variable(var_name, vendor, attrs, &mac_local).is_ok()
        {
            count += 1;
        }
    }

    count
}

#[cfg(not(target_os = "uefi"))]
pub unsafe fn spoof_nvram_mac_addresses(_config: &SpoofConfig) -> u32 {
    0
}

/// Vérifie si un buffer de données ressemble à une adresse MAC unicast valide.
pub fn is_likely_mac_address(data: &[u8]) -> bool {
    if data.len() < 6 {
        return false;
    }
    let mac = &data[..6];
    if mac.iter().all(|&b| b == 0) || mac.iter().all(|&b| b == 0xFF) {
        return false;
    }
    // Bit 0 du premier octet = 1 indique une adresse multicast (sauf 0x01 spécifique)
    if mac[0] & 0x01 != 0 && mac[0] != 0x01 {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_likely_mac_address_validation() {
        assert!(!is_likely_mac_address(&[0, 0, 0, 0, 0, 0]));
        assert!(!is_likely_mac_address(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]));
        assert!(!is_likely_mac_address(&[0x03, 0x11, 0x22, 0x33, 0x44, 0x55])); // multicast
        assert!(is_likely_mac_address(&[0x02, 0xA4, 0x7C, 0x91, 0xE8, 0x3B])); // locally administered unicast
    }
}
