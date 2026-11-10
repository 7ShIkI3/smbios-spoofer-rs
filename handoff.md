# Handoff — `smbios-spoofer-rs` v0.2.0 (UEFI Pre-Boot + Windows Ring 0 Disk Filter)

**Date** : 2026-11-11  
**Dépôt** : `C:\Users\rapha\Documents\Projets\smbios\smbios-spoofer-rs` (`origin/main` @ `2c07ed2`)  
**Machine cible vérifiée** : ASUS TUF Gaming F17 (`FX707ZM`) — NVMe SK hynix `HFM001TD3JX013N` (`1 To`)

---

## 1. Ce qui a été fait (Fait)

### Architecture Dual-Stage complète (UEFI + Windows Ring 0)
1. **Stage 1 — UEFI Pre-Boot (`smbios-spoofer.efi`, 60 KB)** :
   - Patch in-place des tables SMBIOS 2.x & 3.0 en RAM physique avant `ExitBootServices()` :
     - **Type 0** (BIOS Version & Release Date)
     - **Type 1** (System UUID RFC 4122 v4, SerialNumber, SKU)
     - **Type 2** (Baseboard SerialNumber, AssetTag, LocationInChassis)
     - **Type 3** (Chassis SerialNumber, AssetTag, SKU)
     - **Type 4** (Processor SerialNumber, AssetTag, PartNumber `offset 0x22`)
     - **Type 11** (OEM Strings)
     - **Type 17** (Memory Device / RAM Module SerialNumber par barrette DDR4/DDR5)
   - Interception pre-boot des protocoles `EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL` (NVMe Identify Controller `CNS=0x01`, bytes `4..23`) et `EFI_ATA_PASS_THRU_PROTOCOL` (ATA `IDENTIFY DEVICE 0xEC`, words `10..19`).
   - Persistance du profil déterministe **`v9` (`72 octets`, magic `0x534D4249`)** sur l'ESP (`\EFI\SmbiosSpoofer\uuid.dat`) ET publication dans la variable UEFI Runtime NVRAM `SmbiosSpoof` (`NV | BS | RT`, GUID `e3f5a1b2-7c8d-4e9f-a0b1-c2d3e4f5a6b7`).
   - Chaînage automatique vers `\EFI\Microsoft\Boot\bootmgfw.efi` via `LoadImage` + `StartImage`.

2. **Stage 2 — Windows Kernel WDM Disk Filter (`smbios-disk-filter.sys`, 12.5 KB)** :
   - Crate `no_std` dans [`driver/`](file:///C:/Users/rapha/Documents/Projets/smbios/smbios-spoofer-rs/driver/Cargo.toml) (`#![no_main]`, sous-système `NATIVE`, `.reloc` valide `/FIXED:NO`, import `ntoskrnl.exe` via [`driver/ntoskrnl.def`](file:///C:/Users/rapha/Documents/Projets/smbios/smbios-spoofer-rs/driver/ntoskrnl.def)).
   - Charge le profil `v9` (`disk_serial` + `uuid`) au démarrage depuis la NVRAM runtime (`ExGetFirmwareEnvironmentVariable`) ou directement depuis `\Device\HarddiskVolume1..8\EFI\SmbiosSpoofer\uuid.dat` (`ZwOpenFile` + `ZwReadFile`).
   - Attache des Filter Device Objects (`FiDO`) sur :
     - `\Device\Harddisk0\DR0` .. `\Device\Harddisk7\DR7` (`Win32_DiskDrive`, `MSFT_Disk` / `Get-Disk`, `DeviceIoControl`)
     - `\Device\RaidPort0` .. `\Device\RaidPort3` (`\\.\Scsi0:`)
     - `\Device\Spaceport` (`Get-PhysicalDisk` / Storage Spaces)
   - Intercepte `IRP_MJ_DEVICE_CONTROL` avec completion routine (`IoSetCompletionRoutine`) sur :
     - `IOCTL_STORAGE_QUERY_PROPERTY` (`0x002D1400`) : `StorageDeviceProperty (0)` et `StorageDeviceUniqueIdProperty (3)`
     - `IOCTL_STORAGE_PROTOCOL_COMMAND` (`0x002DD380`) & `IOCTL_SCSI_MINIPORT` (`0x0004D008`) : NVMe Identify Controller (`4096B`, bytes `4..23`)
     - `SMART_RCV_DRIVE_DATA` (`0x0007C088`) & `IOCTL_ATA_PASS_THROUGH` (`0x0004D02C`) : ATA Identify word-swapped (`words 10..19`)
     - `IOCTL_SCSI_PASS_THROUGH` (`0x0004D004`) & `IOCTL_SCSI_PASS_THROUGH_DIRECT` (`0x0004D014`) : SCSI VPD `0x80` (Unit Serial) et `0x83` (Device Identification)
   - Préservation du suffixe Namespace ID (`_00000001.`) de `stornvme.sys` sur les numéros de série NVMe de 30 octets (`YF0W826954803M86Y   _00000001.`) et sur les descripteurs VPD `0x83` de 68 octets (`offset 48..68`).
   - Synchronisation automatique en Ring 0 (`DriverEntry`) et via [`install.ps1`](file:///C:/Users/rapha/Documents/Projets/smbios/smbios-spoofer-rs/install.ps1) de :
     - `HKLM\HARDWARE\DEVICEMAP\Scsi\Scsi Port X\...\Logical Unit Id 0\SerialNumber`
     - `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`
     - `HKLM\SYSTEM\CurrentControlSet\Control\SystemInformation\ComputerHardwareId`
     - `HKLM\SOFTWARE\Microsoft\SQMClient\MachineId`
   - Support de `DriverUnload` sécurisé avec compteur atomique `ACTIVE_IRPS` pour permettre la mise à jour à chaud du `.sys`.

3. **Qualité & Vérification** :
   - **134 tests unitaires** (`cargo test --lib`) couvrant les patchs SMBIOS, NVRAM `v9`, et toutes les structures IOCTL disque (`src/win_disk_patch.rs`).
   - **Vérification en conditions réelles** (`.\install.ps1 -Mode status`) : 100 % des identifiants (`System UUID`, `System Serial`, `System SKU`, `Baseboard Serial`, `Chassis Serial`, `Chassis AssetTag`, `Processor Serial`, `BIOS Version/Date`, `MachineGuid`, `HardwareId`, `RAM[0..1]`, `Disk[0] Serial`, `Disk[0] ScsiReg`) sont `[SPOOFED]` et synchronisés.

---

## 2. Décisions techniques (Décisions)

- **Pas de modification des GUIDs GPT sur disque** : modifier les GUIDs de partition GPT de l'ESP ou de la partition OS casse le Boot Configuration Data (`BCD`, erreur `0xc000000e`). Le spoofing disque se fait exclusivement en mémoire (UEFI PassThru + WDM IOCTL/Registry).
- **Service autonome (`Start = SYSTEM_START`, `ErrorControl = Ignore`) hors `LowerFilters`** : `smbios-disk-filter.sys` n'est **jamais** inscrit dans `HKLM\...\Class\{4d36e967-e325-11ce-bfc1-08002be10318}\LowerFilters`. Ainsi, si `testsigning` est désactivé ou si Secure Boot est réactivé, Windows démarre normalement sans `INACCESSIBLE_BOOT_DEVICE` (`0x7B`).
- **Préservation de `Baseboard Model` (`FX707ZM`) et `BIOS Vendor` (`American Megatrends International, LLC.`)** : conservés volontairement dans le profil OEM ASUS pour que les utilitaires constructeurs (Armoury Crate, MyASUS, gestion thermique/ventilateurs) continuent de reconnaître le modèle de châssis tout en ayant tous les numéros de série et UUID spoofés.
- **Signature Test-Mode auto-générée** : [`install.ps1`](file:///C:/Users/rapha/Documents/Projets/smbios/smbios-spoofer-rs/install.ps1) génère et installe un certificat `CN=SmbiosSpooferKernelTest` dans `Cert:\LocalMachine\Root` et `TrustedPublisher`, puis signe `smbios-disk-filter.sys`.

---

## 3. Points d'attention / Contraintes connues (Blocages)

- **Secure Boot & Test Signing** :
  - L'exécution de `smbios-spoofer.efi` (non signé Microsoft UEFI CA) requiert **Secure Boot = Disabled** dans le BIOS.
  - Le chargement du driver noyau `smbios-disk-filter.sys` signé avec un certificat local requiert **`bcdedit /set testsigning on`**.
- **WMI `MSFT_PhysicalDisk` (Storage Spaces Cache)** :
  - Le service `Spaceport` met parfois en cache le `SerialNumber` de la structure interne `SPACES_PhysicalDisk` très tôt au boot (`BOOT_START`) avant `SYSTEM_START`. En revanche, `Win32_DiskDrive`, `Get-Disk` (`MSFT_Disk`), `DeviceIoControl` direct sur `\\.\PhysicalDrive0` et `HKLM\HARDWARE\DEVICEMAP\Scsi` retournent tous la valeur spoofée `YF0W826954803M86Y   _00000001.`.

---

## 4. Pistes d'évolution futures (À faire / Optionnel)

- **Spoofing MAC Address / Network Adapter PnP** : ajouter une option dans `install.ps1` ou le driver pour régénérer une adresse MAC localement administrée synchronisée avec le seed `v9`.
- **Spoofing GPU UUID (NVIDIA/AMD)** : étude d'interception ou de masquage pour les requêtes NVML/DXGI si nécessaire.
- **Signature EV / KDU (Optionnel)** : permettre le chargement du filtre disque sans `testsigning on` si besoin dans le futur.

---

## 5. Commandes de référence rapide

```powershell
# 1. Vérifier le statut complet (SMBIOS + Disque + Registre + ESP + Driver)
.\install.ps1 -Mode status

# 2. Mettre à jour les binaires déjà compilés sans recompiler
.\install.ps1 -Mode install-only

# 3. Générer une toute nouvelle identité matérielle au prochain reboot
.\install.ps1 -Mode reset-uuid

# 4. Désinstaller complètement (boot UEFI + service noyau SmbiosDiskFilter)
.\install.ps1 -Mode uninstall
```
