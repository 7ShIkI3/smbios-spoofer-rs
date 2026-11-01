//! Module de spoofing d'adresse MAC via UEFI Simple Network Protocol.
//!
//! ## Stratégies
//!
//! **A) SNP StationAddress()** — Modification pre-boot de la MAC :
//! - Énumère tous les handles exposant `SimpleNetworkProtocol`
//! - Pour chaque handle : ouvre le protocole, initialise si nécessaire,
//!   puis appelle `station_address(false, Some(&new_mac))`.
//! - Best-effort : loggue les succès/échecs sans bloquer le boot.
//!
//! **B) NVRAM MAC detection** — Détection de patterns MAC dans les variables NVRAM :
//! - Certains firmwares (Dell, Lenovo, HP) cachent les MAC dans des variables NVRAM.
//! - On vérifie des noms de variables connus et on overwrite si un MAC est détecté.
//! - Cette approche est limitée car uefi 0.39 n'expose pas GetNextVariableName
//!   en safe Rust. On utilise une liste statique de noms de variables suspectes.
//!
//! ## Limitation
//! Les changements MAC au niveau UEFI ne survivent PAS à ExitBootServices().
//! Le driver NDIS Windows réinitialise la MAC depuis le hardware.
//! Pour une persistance dans Windows, il faut soit :
//! - Un driver kernel Windows qui hook le NDIS miniport
//! - Une clé de registre `NetworkAddress` dans HKLM\SYSTEM\...\Class\{4D36E972-...}
//! - Un firmware qui respecte la MAC "locally administered" après boot OS

use crate::persistence::SpoofConfig;
use uefi::proto::network::snp::{SimpleNetwork, NetworkState};
use uefi::proto::network::EfiMacAddr;
use uefi::Identify;

/// Spoof l'adresse MAC via le Simple Network Protocol.
///
/// Parcourt tous les handles SNP disponibles et définit une nouvelle adresse MAC
/// "locally administered" pour chacun.
///
/// Retourne le nombre d'interfaces modifiées avec succès.
///
/// # Safety
/// Appelle les fonctions du protocole UEFI SNP. Les modifications sont volatiles
/// (elles ne survivent pas au boot de l'OS).
pub unsafe fn spoof_mac_via_snp(config: &SpoofConfig) -> u32 {
    let mac_local = config.mac_address; // Copie depuis packed struct

    let handles = match uefi::boot::locate_handle_buffer(
        uefi::boot::SearchType::ByProtocol(&SimpleNetwork::GUID),
    ) {
        Ok(h) => h,
        Err(_) => return 0,
    };

    let mut count: u32 = 0;

    // HandleBuffer implémente Deref<Target = [Handle]>
    for &handle in handles.iter() {
        // Ouvrir le protocole SNP sur ce handle
        let snp = match uefi::boot::open_protocol_exclusive::<SimpleNetwork>(handle) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Vérifier l'état du réseau — doit être Started ou Initialized
        let mode = snp.mode();
        if mode.state == NetworkState::STOPPED {
            // Essayer de démarrer l'interface
            if snp.start().is_err() {
                continue;
            }
            if snp.initialize(0, 0).is_err() {
                continue;
            }
        }

        // Construire l'adresse MAC EFI (32 bytes, seuls les 6 premiers sont utilisés)
        let mut efi_mac = EfiMacAddr([0u8; 32]);
        efi_mac.0[0..6].copy_from_slice(&mac_local);

        // Définir la nouvelle adresse (reset=false → on ne restaure pas l'originale)
        if snp.station_address(false, Some(&efi_mac)).is_ok() {
            count += 1;
        }

        // Shutdown propre (certains firmwares peuvent avoir besoin)
        let _ = snp.shutdown();
    }

    count
}

/// Variable NVRAM static — noms de variables firmware suspectes.
const VAR_NET_MAC: &[u16] = &[
    'N' as u16, 'e' as u16, 't' as u16, 'w' as u16, 'o' as u16, 'r' as u16,
    'k' as u16, 'M' as u16, 'a' as u16, 'c' as u16, 'A' as u16, 'd' as u16,
    'd' as u16, 'r' as u16, 'e' as u16, 's' as u16, 's' as u16, 0u16,
];

const VAR_ASSET_TAG: &[u16] = &[
    'A' as u16, 's' as u16, 's' as u16, 'e' as u16, 't' as u16, 'T' as u16,
    'a' as u16, 'g' as u16, 0u16,
];

/// GUID pour les variables réseau globales EDK2.
const NETWORK_GUID: uefi::Guid = uefi::Guid::new(
    [0xD3, 0x90, 0x5A, 0x43], [0xC3, 0xCD], [0x49, 0x48],
    0x8E, 0x31, [0x3C, 0x28, 0xCA, 0x4F, 0xB1, 0x6B],
);

/// GUID vendor Dell pour AssetTag.
const DELL_GUID: uefi::Guid = uefi::Guid::new(
    [0x16, 0x84, 0x1B, 0xDE], [0x5D, 0x89], [0x43, 0x4D],
    0x92, 0x79, [0x8A, 0xA2, 0x2F, 0xEE, 0x48, 0x46],
);

/// Cherche et nettoie les variables NVRAM contenant potentiellement
/// des adresses MAC. Best-effort : les échecs sont silencieux.
///
/// Retourne le nombre de variables modifiées.
///
/// # Safety
/// Appelle les UEFI runtime services pour lire/écrire la NVRAM.
/// Modifier ou supprimer des variables firmware peut avoir des effets
/// secondaires — on est conservatif : on overwrite avec la MAC spoofée
/// au lieu de supprimer.
pub unsafe fn spoof_nvram_mac_addresses(config: &SpoofConfig) -> u32 {
    let mac_local = config.mac_address;
    let mut count: u32 = 0;

    let vendor_nvram = uefi::runtime::VariableVendor(NETWORK_GUID);
    let vendor_dell = uefi::runtime::VariableVendor(DELL_GUID);
    let vendors: &[uefi::runtime::VariableVendor] = &[vendor_nvram, vendor_dell];

    let var_names: &[&[u16]] = &[VAR_NET_MAC, VAR_ASSET_TAG];

    // Essayer de lire chaque variable et vérifier si elle contient un MAC
    for (idx, var_name) in var_names.iter().enumerate() {
        let vendor = &vendors[idx];

        let mut data_buf = [0u8; 64];

        // Convertir le nom u16 en CStr16
        let cstr_name = uefi::CStr16::from_u16_until_nul(var_name)
            .unwrap_or(uefi::cstr16!(""));

        let result = uefi::runtime::get_variable(cstr_name, vendor, &mut data_buf);

        if result.is_ok() {
            // Vérifier si ces données ressemblent à une MAC (6 octets significatifs)
            if is_likely_mac_address(&data_buf) {
                // Overwrite avec la MAC spoofée
                let new_data = mac_local;

                let attrs = uefi::runtime::VariableAttributes::BOOTSERVICE_ACCESS
                    | uefi::runtime::VariableAttributes::NON_VOLATILE;

                let set_result = uefi::runtime::set_variable(
                    cstr_name,
                    vendor,
                    attrs,
                    &new_data,
                );

                if set_result.is_ok() {
                    count += 1;
                }
            }
        }
    }

    count
}

/// Vérifie si un buffer de données ressemble à une adresse MAC.
///
/// Critères :
/// - Au moins 6 octets
/// - Pas tout à zéro
/// - Pas broadcast (FF:FF:FF:FF:FF:FF)
fn is_likely_mac_address(data: &[u8]) -> bool {
    if data.len() < 6 {
        return false;
    }

    // Vérifier les 6 premiers octets
    let mac = &data[..6];

    // Pas tout à zéro
    if mac.iter().all(|&b| b == 0) {
        return false;
    }

    // Pas broadcast
    if mac.iter().all(|&b| b == 0xFF) {
        return false;
    }

    // Pas multicast (bit 0 du premier octet)
    if mac[0] & 0x01 != 0 && mac[0] != 0x01 {
        return false;
    }

    true
}
