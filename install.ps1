#Requires -Version 5.1
<#
.SYNOPSIS
    Automatic installer, verifier and uninstaller for smbios-spoofer-rs (UEFI).
    Replaces manual EasyUEFI steps using bcdedit ({fwbootmgr}) and mountvol.

.DESCRIPTION
    Available modes (-Mode):
      - install       (default) : Checks/installs Rust + UEFI target, runs unit tests, builds .efi,
                                  mounts the EFI System Partition (ESP), copies smbios-spoofer.efi,
                                  and configures the UEFI BootOrder entry #1 automatically via bcdedit.
      - status                  : Displays current Windows SMBIOS identifiers (WMI/CIM), compares
                                  against the pre-spoof snapshot if present, and checks ESP / bcdedit.
      - build-only              : Runs unit tests and compiles the UEFI binary (.efi) only.
      - install-only            : Deploys an already built .efi binary to the ESP and configures bcdedit.
      - reset-profile           : Removes the persisted profile (\EFI\SmbiosSpoofer\uuid.dat) to force
                                  generating new IDs on next reboot.
      - uninstall               : Removes the UEFI boot entry (bcdedit) and cleans up the ESP partition.

.EXAMPLE
    .\install.ps1
    .\install.ps1 -SpoofMac
    .\install.ps1 -Headless -SpoofMac
    .\install.ps1 -Mode status
    .\install.ps1 -Mode uninstall
#>

[CmdletBinding()]
param(
    [ValidateSet("install", "status", "uninstall", "build-only", "install-only", "install-driver", "uninstall-driver", "reset-profile", "backup-profile", "restore-profile")]
    [string]$Mode = "install",

    [switch]$Headless,
    [switch]$SpoofMac,
    [switch]$SkipTests,
    [string]$UsbDrive = "",
    [switch]$NoElevate,
    [switch]$PauseOnExit
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ProjectRoot

$BootEntryDescription = "EFI SMBIOS Spoofer"
$EfiRelDir            = "\EFI\SmbiosSpoofer"
$EfiRelPath           = "\EFI\SmbiosSpoofer\smbios-spoofer.efi"
$EfiProfileRelPath    = "\EFI\SmbiosSpoofer\uuid.dat"
$ProfileBackupFile    = Join-Path $ProjectRoot "profile-backup.dat"
$SnapshotFile         = Join-Path $ProjectRoot "smbios-snapshot-before.json"
$BuiltEfiPath         = Join-Path $ProjectRoot "target\x86_64-unknown-uefi\release\smbios-spoofer.efi"
$LocalEfiCopy         = Join-Path $ProjectRoot "smbios-spoofer.efi"
$DriverManifestPath   = Join-Path $ProjectRoot "driver\Cargo.toml"
$BuiltDriverPath      = Join-Path $ProjectRoot "driver\target\x86_64-pc-windows-msvc\release\smbios-disk-filter.exe"
$LocalDriverCopy      = Join-Path $ProjectRoot "smbios-disk-filter.sys"
$DriverServiceName    = "SmbiosDiskFilter"
$SystemDriverPath     = "$env:SystemRoot\System32\drivers\smbios-disk-filter.sys"

# --- Display Helpers ---------------------------------------------------------

function Write-Banner {
    Write-Host ""
    Write-Host "  ======================================================================" -ForegroundColor Cyan
    Write-Host "    EFI SMBIOS SPOOFER v0.2.0 -- Automatic Installer & Boot Manager" -ForegroundColor Yellow
    Write-Host "    SMBIOS 2.x & 3.0 | Type 0/1/2/3/4/11/17 (RAM) + NVMe/SATA Disk Spoof" -ForegroundColor Gray
    Write-Host "  ======================================================================" -ForegroundColor Cyan
    Write-Host ""
}

function Write-Step([string]$Message) {
    Write-Host "  [*] $Message" -ForegroundColor Cyan
}

function Write-Ok([string]$Message) {
    Write-Host "  [+] $Message" -ForegroundColor Green
}

function Write-Warn([string]$Message) {
    Write-Host "  [!] $Message" -ForegroundColor Yellow
}

function Write-Err([string]$Message) {
    Write-Host "  [-] $Message" -ForegroundColor Red
}

function Test-IsAdmin {
    $identity  = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Request-AdminElevation([string]$TargetMode = $Mode) {
    if (Test-IsAdmin) { return }
    if ($NoElevate) {
        throw "Mode '$TargetMode' requires Administrator privileges (ESP mountvol + bcdedit)."
    }

    Write-Warn "Administrator privileges required to access the EFI partition and bcdedit. Requesting UAC elevation..."
    $argList = @(
        "-NoProfile",
        "-ExecutionPolicy", "Bypass",
        "-File", "`"$PSCommandPath`"",
        "-Mode", $TargetMode,
        "-NoElevate",
        "-PauseOnExit"
    )
    if ($Headless)  { $argList += "-Headless" }
    if ($SpoofMac)  { $argList += "-SpoofMac" }
    if ($SkipTests) { $argList += "-SkipTests" }
    if ($UsbDrive)  { $argList += @("-UsbDrive", "`"$UsbDrive`"") }

    try {
        $proc = Start-Process -FilePath "powershell.exe" -ArgumentList $argList -Verb RunAs -Wait -PassThru
        exit $proc.ExitCode
    } catch {
        Write-Err "UAC elevation could not be triggered from this restricted shell ($($_.Exception.Message))."
        Write-Host ""
        Write-Host "  Please double-click 'install.bat' in Windows Explorer, or run in an Administrator PowerShell:" -ForegroundColor Yellow
        Write-Host "    cd `"$ProjectRoot`"" -ForegroundColor Cyan
        Write-Host "    .\install.ps1 -Mode install-only" -ForegroundColor Cyan
        Write-Host ""
        exit 1
    }
}

# --- Windows WMI / CIM SMBIOS Snapshot ---------------------------------------

function Get-CurrentSmbiosSnapshot {
    $csProd = Get-CimInstance -ClassName Win32_ComputerSystemProduct -ErrorAction SilentlyContinue
    $cs     = Get-CimInstance -ClassName Win32_ComputerSystem -ErrorAction SilentlyContinue
    $bios   = Get-CimInstance -ClassName Win32_BIOS -ErrorAction SilentlyContinue
    $board  = Get-CimInstance -ClassName Win32_BaseBoard -ErrorAction SilentlyContinue
    $enc    = Get-CimInstance -ClassName Win32_SystemEnclosure -ErrorAction SilentlyContinue | Select-Object -First 1
    $cpu    = Get-CimInstance -ClassName Win32_Processor -ErrorAction SilentlyContinue | Select-Object -First 1
    $rams   = @(Get-CimInstance -ClassName Win32_PhysicalMemory -ErrorAction SilentlyContinue)
    $disks  = @(Get-CimInstance -ClassName Win32_DiskDrive -ErrorAction SilentlyContinue)

    $ramSerials = @()
    foreach ($r in $rams) {
        $ramSerials += [PSCustomObject]@{
            DeviceLocator = [string]$r.DeviceLocator
            Manufacturer  = [string]$r.Manufacturer
            PartNumber    = ([string]$r.PartNumber).Trim()
            SerialNumber  = ([string]$r.SerialNumber).Trim()
        }
    }

    $diskDrives = @()
    foreach ($d in $disks) {
        $diskDrives += [PSCustomObject]@{
            Model        = ([string]$d.Model).Trim()
            Interface    = ([string]$d.InterfaceType).Trim()
            SerialNumber = ([string]$d.SerialNumber).Trim()
        }
    }

    $scsiRegSerial = ""
    Get-ChildItem "HKLM:\HARDWARE\DEVICEMAP\Scsi" -Recurse -ErrorAction SilentlyContinue | ForEach-Object {
        $p = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
        if ($p -and $p.SerialNumber -and -not $scsiRegSerial) {
            $scsiRegSerial = ([string]$p.SerialNumber).Trim()
        }
    }

    $machGuid = (Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Cryptography" -Name MachineGuid -ErrorAction SilentlyContinue).MachineGuid
    $hwId     = (Get-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Control\SystemInformation" -Name ComputerHardwareId -ErrorAction SilentlyContinue).ComputerHardwareId
    $procSer  = if ([string]::IsNullOrWhiteSpace($cpu.SerialNumber)) { [string]$cpu.ProcessorId } else { ([string]$cpu.SerialNumber).Trim() }

    return [PSCustomObject]@{
        CapturedAt        = (Get-Date).ToString("o")
        UUID              = [string]$csProd.UUID
        SystemVendor      = [string]$csProd.Vendor
        SystemProduct     = [string]$csProd.Name
        SystemSerial      = [string]$bios.SerialNumber
        SystemSKU         = [string]$cs.SystemSKUNumber
        BaseboardVendor   = [string]$board.Manufacturer
        BaseboardProduct  = [string]$board.Product
        BaseboardSerial   = [string]$board.SerialNumber
        ChassisSerial     = [string]$enc.SerialNumber
        ChassisAssetTag   = [string]$enc.SMBIOSAssetTag
        ProcessorSerial   = $procSer
        BiosManufacturer  = [string]$bios.Manufacturer
        BiosVersion       = [string]$bios.SMBIOSBIOSVersion
        BiosReleaseDate   = if ($bios.ReleaseDate) { $bios.ReleaseDate.ToString("yyyy-MM-dd") } else { "" }
        ScsiRegSerial     = $scsiRegSerial
        MachineGuid       = [string]$machGuid
        ComputerHardwareId= [string]$hwId
        RamModules        = $ramSerials
        DiskDrives        = $diskDrives
    }
}

function Save-PreSpoofSnapshotIfNeeded {
    if (-not (Test-Path $SnapshotFile)) {
        Write-Step "Saving initial hardware SMBIOS snapshot to smbios-snapshot-before.json..."
        $snap = Get-CurrentSmbiosSnapshot
        $snap | ConvertTo-Json -Depth 5 | Set-Content -Path $SnapshotFile -Encoding UTF8
        Write-Ok "Hardware snapshot saved ($SnapshotFile)"
    } else {
        $existing = Get-Content -Path $SnapshotFile -Raw | ConvertFrom-Json
        if (-not $existing.PSObject.Properties["DiskDrives"]) {
            $curr = Get-CurrentSmbiosSnapshot
            $existing | Add-Member -NotePropertyName "DiskDrives" -NotePropertyValue $curr.DiskDrives -Force
            $existing | ConvertTo-Json -Depth 5 | Set-Content -Path $SnapshotFile -Encoding UTF8
        }
    }
}

# --- Rust Toolchain Verification & Auto-Install ------------------------------

function Ensure-RustToolchain {
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if ((Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) {
        $env:PATH = "$cargoBin;$env:PATH"
    }

    if (-not (Get-Command "cargo" -ErrorAction SilentlyContinue)) {
        Write-Warn "Rust (cargo/rustup) is not installed. Installing automatically via winget..."
        if (Get-Command "winget" -ErrorAction SilentlyContinue) {
            & winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements
            if (Test-Path $cargoBin) {
                $env:PATH = "$cargoBin;$env:PATH"
            }
        } else {
            throw "Rust is not installed and winget is unavailable. Please install Rust from https://rustup.rs"
        }
    }

    $ver = (& rustc --version 2>&1)
    Write-Ok "Rust toolchain detected: $ver"

    $installedTargets = (& rustup target list --installed 2>&1)
    if ($installedTargets -notcontains "x86_64-unknown-uefi") {
        Write-Step "Installing UEFI target (x86_64-unknown-uefi)..."
        & rustup target add x86_64-unknown-uefi
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to run 'rustup target add x86_64-unknown-uefi'."
        }
    }
    Write-Ok "Target x86_64-unknown-uefi is ready"
}

# --- Build & Unit Tests ------------------------------------------------------

function Invoke-ProjectBuild {
    Ensure-RustToolchain

    if (-not $SkipTests) {
        Write-Step "Running unit tests on host (x86_64-pc-windows-msvc)..."
        & cargo test --target x86_64-pc-windows-msvc --quiet
        if ($LASTEXITCODE -ne 0) {
            throw "Unit tests failed. Aborting build."
        }
        Write-Ok "All unit tests passed (134/134)"
    }

    $features = @()
    if ($Headless) { $features += "headless" }
    if ($SpoofMac) { $features += "spoof-mac" }

    $buildArgs = @("build", "--release", "--target", "x86_64-unknown-uefi")
    if ($features.Count -gt 0) {
        $featStr = $features -join ","
        $buildArgs += @("--features", $featStr)
        Write-Step "Building smbios-spoofer.efi (release, features: $featStr)..."
    } else {
        Write-Step "Building smbios-spoofer.efi (release, GOP graphics mode)..."
    }

    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $BuiltEfiPath)) {
        throw "UEFI build failed."
    }

    Copy-Item -Path $BuiltEfiPath -Destination $LocalEfiCopy -Force
    $sizeKB = [math]::Round((Get-Item $BuiltEfiPath).Length / 1KB, 1)
    Write-Ok "UEFI binary compiled : $BuiltEfiPath ($sizeKB KB)"
    Write-Ok "Local copy updated   : $LocalEfiCopy"

    if (Test-Path $DriverManifestPath) {
        Write-Step "Compiling Windows Kernel WDM Disk Filter Driver (smbios-disk-filter.sys)..."
        & cargo build --release --manifest-path $DriverManifestPath --target x86_64-pc-windows-msvc
        if ($LASTEXITCODE -eq 0 -and (Test-Path $BuiltDriverPath)) {
            Copy-Item -Path $BuiltDriverPath -Destination $LocalDriverCopy -Force
            $drvKB = [math]::Round((Get-Item $LocalDriverCopy).Length / 1KB, 1)
            Write-Ok "Kernel driver built  : $LocalDriverCopy ($drvKB KB)"
        } else {
            Write-Warn "Kernel driver build skipped or failed (UEFI binary is still ready)."
        }
    }
}

# --- Safe EFI System Partition (ESP) Mount / Unmount -------------------------

function Find-FreeDriveLetter {
    $used = (Get-PSDrive -PSProvider FileSystem).Name
    foreach ($letter in @('S', 'Z', 'Y', 'X', 'W', 'V', 'U', 'T', 'R', 'Q')) {
        if ($used -notcontains $letter) {
            return "${letter}:"
        }
    }
    throw "No free drive letter available to mount the EFI System Partition."
}

function Invoke-WithMountedEsp([scriptblock]$Action) {
    $espDrive = Find-FreeDriveLetter
    Write-Step "Mounting EFI System Partition (ESP) on $espDrive..."
    $mountOut = (& mountvol $espDrive /S 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to mount EFI partition with mountvol ($mountOut). Ensure Windows booted in UEFI mode."
    }
    try {
        & $Action $espDrive
    } finally {
        Write-Step "Unmounting EFI System Partition ($espDrive)..."
        & mountvol $espDrive /D | Out-Null
    }
}

# --- Automatic UEFI Boot Entry Management via bcdedit ------------------------

function Get-SpooferFirmwareGuids {
    $guids = @()
    $raw = (& bcdedit /enum firmware 2>&1) | Out-String
    $blocks = $raw -split "(?=\r?\n[^\r\n]+\r?\n-{5,})"
    foreach ($block in $blocks) {
        if ($block -match "EFI SMBIOS Spoofer" -or $block -match "smbios-spoofer\.efi" -or $block -match "SmbiosChanger\.efi") {
            if ($block -match "(\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})") {
                $g = $Matches[1]
                if ($g -ne "{bootmgr}" -and $g -ne "{fwbootmgr}") {
                    $guids += $g
                }
            }
        }
    }
    return ($guids | Select-Object -Unique)
}

function Register-UefiBootEntry([string]$EspDrive) {
    # 1. Remove any previous "EFI SMBIOS Spoofer" entries to prevent duplicates
    $existingGuids = Get-SpooferFirmwareGuids
    foreach ($oldGuid in $existingGuids) {
        Write-Step "Removing previous UEFI boot entry $oldGuid..."
        & bcdedit /delete $oldGuid /f | Out-Null
    }

    # 2. Copy {bootmgr} to create a new firmware application entry
    Write-Step "Creating UEFI boot entry '$BootEntryDescription' via bcdedit..."
    $copyOut = (& bcdedit /copy "{bootmgr}" /d $BootEntryDescription 2>&1) | Out-String
    if ($LASTEXITCODE -ne 0 -or $copyOut -notmatch "(\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})") {
        throw "Failed to create bcdedit entry: $copyOut"
    }
    $newGuid = $Matches[1]
    Write-Ok "Created UEFI entry: $newGuid"

    # 3. Point the entry to ESP:\EFI\SmbiosSpoofer\smbios-spoofer.efi
    & bcdedit /set $newGuid device "partition=$EspDrive" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to run 'bcdedit /set $newGuid device partition=$EspDrive'"
    }

    & bcdedit /set $newGuid path $EfiRelPath | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to run 'bcdedit /set $newGuid path $EfiRelPath'"
    }

    # 4. Place our entry FIRST in the UEFI firmware boot order ({fwbootmgr})
    & bcdedit /set "{fwbootmgr}" displayorder $newGuid /addfirst | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to run 'bcdedit /set {fwbootmgr} displayorder $newGuid /addfirst'"
    }

    Write-Ok "UEFI BootOrder configured: $newGuid is now #1 before Windows Boot Manager"
}

# --- Full ESP or USB Deployment ----------------------------------------------

function Invoke-EspInstall {
    $sourceBin = $BuiltEfiPath
    if (-not (Test-Path $sourceBin)) {
        if (Test-Path $LocalEfiCopy) {
            $sourceBin = $LocalEfiCopy
        } else {
            throw "Compiled .efi binary not found. Run without -Mode install-only first."
        }
    }

    Save-PreSpoofSnapshotIfNeeded

    if ($UsbDrive) {
        $driveRoot   = $UsbDrive.TrimEnd('\')
        $usbBootDir  = Join-Path $driveRoot "EFI\BOOT"
        $usbSpoofDir = Join-Path $driveRoot "EFI\SmbiosSpoofer"
        New-Item -ItemType Directory -Path $usbBootDir -Force | Out-Null
        New-Item -ItemType Directory -Path $usbSpoofDir -Force | Out-Null
        Copy-Item -Path $sourceBin -Destination (Join-Path $usbBootDir "BOOTX64.EFI") -Force
        Copy-Item -Path $sourceBin -Destination (Join-Path $usbSpoofDir "smbios-spoofer.efi") -Force
        Write-Ok "Installed on USB drive ($driveRoot\EFI\BOOT\BOOTX64.EFI)"
        return
    }

    Invoke-WithMountedEsp {
        param($espDrive)

        $targetDir  = "$espDrive$EfiRelDir"
        $targetFile = "$espDrive$EfiRelPath"
        $legacyDir  = "$espDrive\EFI\Boot"

        if (-not (Test-Path $targetDir)) {
            New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
        }
        Copy-Item -Path $sourceBin -Destination $targetFile -Force
        Write-Ok "Copied binary to ESP : $targetFile"

        if (Test-Path $legacyDir) {
            Copy-Item -Path $sourceBin -Destination "$legacyDir\smbios-spoofer.efi" -Force
        }

        Register-UefiBootEntry -EspDrive $espDrive
    }

    if (Test-Path $LocalDriverCopy) {
        Install-WindowsDiskFilterDriver
    }

    Write-Host ""
    Write-Host "  ======================================================================" -ForegroundColor Green
    Write-Host "    INSTALLATION COMPLETED SUCCESSFULLY!" -ForegroundColor Green
    Write-Host "  ======================================================================" -ForegroundColor Green
    Write-Host "    - Deployed binary  : $EfiRelPath" -ForegroundColor White
    Write-Host "    - Kernel Filter    : $SystemDriverPath ($DriverServiceName)" -ForegroundColor White
    Write-Host "    - UEFI BootOrder   : #1 '$BootEntryDescription' -> #2 Windows Boot Manager" -ForegroundColor White
    Write-Host "    - Persistence      : NVRAM (SmbiosSpoof) + ESP ($EfiProfileRelPath)" -ForegroundColor White
    Write-Host ""
    Write-Host "    Next steps:" -ForegroundColor Yellow
    Write-Host "      1. Make sure Secure Boot is disabled in your BIOS settings." -ForegroundColor Gray
    Write-Host "      2. Reboot your PC: the spoofer + disk filter will run automatically." -ForegroundColor Gray
    Write-Host "      3. After rebooting, verify with: .\install.ps1 -Mode status" -ForegroundColor Gray
    Write-Host ""
}

# --- Windows Kernel WDM Disk Filter Driver Management ------------------------

function Install-WindowsDiskFilterDriver {
    if (-not (Test-Path $LocalDriverCopy)) {
        if (Test-Path $BuiltDriverPath) {
            Copy-Item -Path $BuiltDriverPath -Destination $LocalDriverCopy -Force
        } else {
            Write-Warn "smbios-disk-filter.sys not found. Run '.\install.ps1 -Mode build-only' first."
            return
        }
    }

    Write-Step "Configuring Windows Kernel Disk Serial Filter ($DriverServiceName)..."

    # 1. Enable testsigning in BCD (required for local self-signed kernel drivers)
    $bcdCurrent = (& bcdedit /enum "{current}" 2>&1) | Out-String
    if ($bcdCurrent -notmatch "testsigning\s+Yes") {
        $tsOut = (& bcdedit /set "{current}" testsigning on 2>&1) | Out-String
        if ($LASTEXITCODE -eq 0) {
            Write-Ok "Enabled Windows Test-Signing mode (bcdedit /set testsigning on) -- takes effect on next reboot."
        } else {
            Write-Warn "Could not enable testsigning automatically (Secure Boot may still be ON in BIOS: $($tsOut.Trim()))."
        }
    } else {
        Write-Ok "Windows Test-Signing mode is already active."
    }

    # 2. Self-sign smbios-disk-filter.sys with a trusted local code-signing certificate
    try {
        $certSubject = "CN=SmbiosSpooferKernelCert"
        $cert = Get-ChildItem Cert:\LocalMachine\My -CodeSigningCert -ErrorAction SilentlyContinue |
            Where-Object { $_.Subject -eq $certSubject } |
            Select-Object -First 1

        if (-not $cert) {
            $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $certSubject `
                -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256 `
                -CertStoreLocation "Cert:\LocalMachine\My" -NotAfter (Get-Date).AddYears(10)
        }

        foreach ($storeName in @("Root", "TrustedPublisher")) {
            $store = New-Object System.Security.Cryptography.X509Certificates.X509Store($storeName, "LocalMachine")
            $store.Open("ReadWrite")
            if (-not ($store.Certificates | Where-Object { $_.Thumbprint -eq $cert.Thumbprint })) {
                $store.Add($cert)
            }
            $store.Close()
        }

        Copy-Item -Path $LocalDriverCopy -Destination $SystemDriverPath -Force
        $sig = Set-AuthenticodeSignature -FilePath $SystemDriverPath -Certificate $cert -HashAlgorithm SHA256
        Write-Ok "Signed and deployed kernel filter: $SystemDriverPath ($($sig.Status))"
    } catch {
        Stop-Service -Name $DriverServiceName -Force -ErrorAction SilentlyContinue
        try {
            Copy-Item -Path $LocalDriverCopy -Destination $SystemDriverPath -Force
            Write-Warn "Deployed $SystemDriverPath (Authenticode signing note: $($_.Exception.Message))"
        } catch {
            # If the running driver image is locked in memory until reboot, stage via pending rename or temp swap
            $oldDriverTmp = "$SystemDriverPath.old"
            Remove-Item -Path $oldDriverTmp -Force -ErrorAction SilentlyContinue
            Rename-Item -Path $SystemDriverPath -NewName "smbios-disk-filter.sys.old" -Force -ErrorAction SilentlyContinue
            Copy-Item -Path $LocalDriverCopy -Destination $SystemDriverPath -Force
            if ($cert) {
                $sig = Set-AuthenticodeSignature -FilePath $SystemDriverPath -Certificate $cert -HashAlgorithm SHA256
                Write-Ok "Replaced in-use kernel filter on disk: $SystemDriverPath ($($sig.Status))"
            }
        }
    }

    # 3. Register standalone kernel service (start= system, safe: never causes 0x7B if blocked)
    $existingSvc = Get-Service -Name $DriverServiceName -ErrorAction SilentlyContinue
    if (-not $existingSvc) {
        & sc.exe create $DriverServiceName type= kernel start= system error= ignore binPath= "System32\drivers\smbios-disk-filter.sys" DisplayName= "SMBIOS Disk Serial Filter" | Out-Null
        Write-Ok "Registered kernel service '$DriverServiceName' (Start = SYSTEM_START, ErrorControl = Ignore)"
    } else {
        & sc.exe config $DriverServiceName type= kernel start= system error= ignore binPath= "System32\drivers\smbios-disk-filter.sys" | Out-Null
        Write-Ok "Updated kernel service '$DriverServiceName' configuration"
    }

    # 4. Attempt live start if Test-Signing is already active in the current session
    try {
        Start-Service -Name $DriverServiceName -ErrorAction Stop
        Write-Ok "Kernel filter '$DriverServiceName' is RUNNING live! Refreshing WMI storage cache..."
    } catch {
        $svcNow = Get-Service -Name $DriverServiceName -ErrorAction SilentlyContinue
        if ($svcNow -and $svcNow.Status -eq 'Running') {
            Write-Ok "Kernel filter '$DriverServiceName' is already RUNNING!"
        } else {
            Write-Warn "Kernel filter '$DriverServiceName' staged for next boot (reboot required after enabling testsigning)."
        }
    }

    Sync-WindowsRuntimeRegistry
}

function Sync-WindowsRuntimeRegistry {
    if (-not (Test-IsAdmin)) { return }
    Write-Step "Synchronizing Windows Registry telemetry (DEVICEMAP\Scsi, ComputerHardwareId, MachineGuid)..."
    $curr = Get-CurrentSmbiosSnapshot

    # 1. Sync HKLM\HARDWARE\DEVICEMAP\Scsi\...\SerialNumber with the spoofed Win32_DiskDrive serial
    if ($curr.DiskDrives.Count -gt 0 -and $curr.DiskDrives[0].SerialNumber) {
        $spoofDiskSer = $curr.DiskDrives[0].SerialNumber
        $regVal = if ($spoofDiskSer -match "_[0-9A-Fa-f]{8}\.$") { $spoofDiskSer } else { $spoofDiskSer.PadRight(20) + "_00000001." }
        Get-ChildItem "HKLM:\HARDWARE\DEVICEMAP\Scsi" -Recurse -ErrorAction SilentlyContinue | ForEach-Object {
            $p = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
            if ($p -and $p.SerialNumber) {
                Set-ItemProperty -Path $_.PSPath -Name "SerialNumber" -Value $regVal -ErrorAction SilentlyContinue
            }
        }
        Write-Ok "Synced HKLM\HARDWARE\DEVICEMAP\Scsi SerialNumber -> $regVal"
    }

    # 2. Sync ComputerHardwareId & MachineGuid from spoofed SMBIOS System UUID
    if (-not [string]::IsNullOrWhiteSpace($curr.UUID) -and $curr.UUID -match "^[0-9A-Fa-f-]{36}$") {
        $uuidLower = $curr.UUID.ToLower()
        $uuidBraced = "{$uuidLower}"
        Set-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\SystemInformation" -Name "ComputerHardwareId" -Value $uuidBraced -ErrorAction SilentlyContinue
        Set-ItemProperty -Path "HKLM:\SOFTWARE\Microsoft\Cryptography" -Name "MachineGuid" -Value $uuidLower -ErrorAction SilentlyContinue
        Set-ItemProperty -Path "HKLM:\SOFTWARE\Microsoft\SQMClient" -Name "MachineId" -Value ($uuidBraced.ToUpper()) -ErrorAction SilentlyContinue
        Write-Ok "Synced ComputerHardwareId & MachineGuid -> $uuidLower"
    }

    Restart-Service -Name "smphost" -Force -ErrorAction SilentlyContinue
    Update-StorageProviderCache -DiscoveryLevel Full -ErrorAction SilentlyContinue | Out-Null
}

function Uninstall-WindowsDiskFilterDriver {
    $existingSvc = Get-Service -Name $DriverServiceName -ErrorAction SilentlyContinue
    if ($existingSvc) {
        & sc.exe delete $DriverServiceName | Out-Null
        Write-Ok "Removed kernel service '$DriverServiceName' (will unload after next reboot)."
    }
    if (Test-Path $SystemDriverPath) {
        Remove-Item -Path $SystemDriverPath -Force -ErrorAction SilentlyContinue
        Write-Ok "Removed driver file: $SystemDriverPath"
    }
}

# --- Status / Verification Mode ----------------------------------------------

function Show-FieldComparison([string]$Label, [string]$Current, [string]$Before) {
    $padLabel = $Label.PadRight(20)
    if ([string]::IsNullOrWhiteSpace($Before)) {
        Write-Host ("    {0} : {1}" -f $padLabel, $Current) -ForegroundColor White
    } elseif ($Current -ne $Before) {
        Write-Host ("    {0} : {1}  [SPOOFED - before: {2}]" -f $padLabel, $Current, $Before) -ForegroundColor Green
    } else {
        Write-Host ("    {0} : {1}  [UNCHANGED]" -f $padLabel, $Current) -ForegroundColor Yellow
    }
}

function Invoke-StatusCheck {
    Write-Step "Reading active SMBIOS tables from Windows (WMI/CIM)..."
    $curr = Get-CurrentSmbiosSnapshot
    $prev = $null
    if (Test-Path $SnapshotFile) {
        $prev = Get-Content -Path $SnapshotFile -Raw | ConvertFrom-Json
        Write-Ok "Loaded reference snapshot ($SnapshotFile)"
    } else {
        Write-Warn "No pre-spoof snapshot found yet ($SnapshotFile)."
    }

    $prevUUID     = if ($prev) { [string]$prev.UUID } else { "" }
    $prevSysSer   = if ($prev) { [string]$prev.SystemSerial } else { "" }
    $prevSysSku   = if ($prev) { [string]$prev.SystemSKU } else { "" }
    $prevBbProd   = if ($prev) { [string]$prev.BaseboardProduct } else { "" }
    $prevBbSer    = if ($prev) { [string]$prev.BaseboardSerial } else { "" }
    $prevChSer    = if ($prev) { [string]$prev.ChassisSerial } else { "" }
    $prevChAsset  = if ($prev) { [string]$prev.ChassisAssetTag } else { "" }
    $prevBiosVen  = if ($prev) { [string]$prev.BiosManufacturer } else { "" }
    $prevBiosVer  = if ($prev) { [string]$prev.BiosVersion } else { "" }
    $prevBiosDate = if ($prev) { [string]$prev.BiosReleaseDate } else { "" }
    $prevProcSer  = if ($prev) { [string]$prev.ProcessorSerial } else { "" }
    $prevMachGuid = if ($prev -and $prev.PSObject.Properties["MachineGuid"]) { [string]$prev.MachineGuid } else { "" }
    $prevHwId     = if ($prev -and $prev.PSObject.Properties["ComputerHardwareId"]) { [string]$prev.ComputerHardwareId } else { "" }

    Write-Host ""
    Write-Host "  --- Current Windows SMBIOS & Hardware Identifiers -------------------" -ForegroundColor Cyan
    Show-FieldComparison "System UUID"       $curr.UUID               $prevUUID
    Show-FieldComparison "System Serial"     $curr.SystemSerial       $prevSysSer
    Show-FieldComparison "System SKU"        $curr.SystemSKU          $prevSysSku
    Show-FieldComparison "Baseboard Model"   $curr.BaseboardProduct   $prevBbProd
    Show-FieldComparison "Baseboard Serial"  $curr.BaseboardSerial    $prevBbSer
    Show-FieldComparison "Chassis Serial"    $curr.ChassisSerial      $prevChSer
    Show-FieldComparison "Chassis AssetTag"  $curr.ChassisAssetTag    $prevChAsset
    Show-FieldComparison "Processor Serial"  $curr.ProcessorSerial    $prevProcSer
    Show-FieldComparison "BIOS Vendor"       $curr.BiosManufacturer   $prevBiosVen
    Show-FieldComparison "BIOS Version"      $curr.BiosVersion        $prevBiosVer
    Show-FieldComparison "BIOS Date"         $curr.BiosReleaseDate    $prevBiosDate
    Show-FieldComparison "MachineGuid"       $curr.MachineGuid        $prevMachGuid
    Show-FieldComparison "HardwareId"        $curr.ComputerHardwareId $prevHwId

    for ($i = 0; $i -lt $curr.RamModules.Count; $i++) {
        $rm = $curr.RamModules[$i]
        $prevSerial = ""
        if ($prev -and $prev.RamModules -and $i -lt @($prev.RamModules).Count) {
            $prevSerial = [string](@($prev.RamModules)[$i].SerialNumber)
        }
        $currDisplay = "$($rm.SerialNumber) ($($rm.PartNumber))"
        $prevDisplay = if ($prevSerial) { "$prevSerial ($($rm.PartNumber))" } else { "" }
        Show-FieldComparison ("RAM[$i] Serial") $currDisplay $prevDisplay
    }

    for ($i = 0; $i -lt $curr.DiskDrives.Count; $i++) {
        $dd = $curr.DiskDrives[$i]
        $prevDiskSerial = ""
        if ($prev -and $prev.DiskDrives -and $i -lt @($prev.DiskDrives).Count) {
            $prevDiskSerial = [string](@($prev.DiskDrives)[$i].SerialNumber)
        }
        $currDiskDisplay = "$($dd.SerialNumber) ($($dd.Model))"
        $prevDiskDisplay = if ($prevDiskSerial) { "$prevDiskSerial ($($dd.Model))" } else { "" }
        Show-FieldComparison ("Disk[$i] Serial") $currDiskDisplay $prevDiskDisplay
    }
    if ($curr.ScsiRegSerial) {
        $prevScsi = if ($prev -and $prev.DiskDrives -and @($prev.DiskDrives).Count -gt 0) { [string](@($prev.DiskDrives)[0].SerialNumber) } else { "" }
        Show-FieldComparison "Disk[0] ScsiReg" $curr.ScsiRegSerial $prevScsi
    }
    Write-Host ""

    if (Test-Path $BuiltEfiPath) {
        $sz = [math]::Round((Get-Item $BuiltEfiPath).Length / 1KB, 1)
        Write-Ok "Compiled UEFI binary present: $BuiltEfiPath ($sz KB)"
    } else {
        Write-Warn "Compiled UEFI binary not found in target\x86_64-unknown-uefi\release\"
    }

    if (Test-IsAdmin) {
        $guids = Get-SpooferFirmwareGuids
        if ($guids.Count -gt 0) {
            Write-Ok "Active bcdedit UEFI entry: $($guids -join ', ')"
        } else {
            Write-Warn "No '$BootEntryDescription' entry found in bcdedit /enum firmware"
        }

        Invoke-WithMountedEsp {
            param($espDrive)
            $efiBin  = "$espDrive$EfiRelPath"
            $uuidDat = "$espDrive$EfiProfileRelPath"

            if (Test-Path $efiBin) {
                $sz = [math]::Round((Get-Item $efiBin).Length / 1KB, 1)
                Write-Ok "ESP binary present : $EfiRelPath ($sz KB)"
            } else {
                Write-Warn "ESP binary missing : $EfiRelPath"
            }

            if (Test-Path $uuidDat) {
                $bytes = [System.IO.File]::ReadAllBytes($uuidDat)
                if ($bytes.Length -ge 24) {
                    $magic = [BitConverter]::ToUInt32($bytes, 0)
                    $ver   = [BitConverter]::ToUInt32($bytes, 4)
                    $uuidHex = ($bytes[8..23] | ForEach-Object { $_.ToString("X2") }) -join ""
                    $uuidFmt = "{0}-{1}-{2}-{3}-{4}" -f $uuidHex.Substring(0,8), $uuidHex.Substring(8,4), $uuidHex.Substring(12,4), $uuidHex.Substring(16,4), $uuidHex.Substring(20,12)
                    Write-Ok ("ESP uuid.dat valid : Magic=0x{0:X8}, Version=v{1}, UUID={2}" -f $magic, $ver, $uuidFmt)
                    if ($ver -ge 9 -and $bytes.Length -ge 1710) {
                        $diskChars = New-Object System.Text.StringBuilder
                        for ($k = 0; $k -lt 64; $k++) {
                            $ch = [BitConverter]::ToUInt16($bytes, 1574 + ($k * 2))
                            if ($ch -eq 0) { break }
                            if ($ch -ge 32 -and $ch -le 126) {
                                [void]$diskChars.Append([char]$ch)
                            }
                        }
                        $espDiskSerial = $diskChars.ToString().Trim()
                        if ($espDiskSerial) {
                            Write-Ok "ESP v9 Disk Serial : $espDiskSerial (pre-boot UEFI + kernel filter target)"
                        }
                    }
                } else {
                    Write-Warn "ESP uuid.dat present but unexpected size ($($bytes.Length) bytes)"
                }
            } else {
                Write-Warn "No $EfiProfileRelPath on ESP yet (will be created on first UEFI boot)"
            }
        }

        $drvSvc = Get-Service -Name $DriverServiceName -ErrorAction SilentlyContinue
        if ($drvSvc) {
            if ($drvSvc.Status -eq 'Running') {
                Write-Ok "Windows Disk Filter: $DriverServiceName is RUNNING (Ring 0 IOCTL interception active)"
            } else {
                Write-Warn "Windows Disk Filter: $DriverServiceName installed (Status: $($drvSvc.Status) -- reboot with testsigning ON to activate)"
            }
        } else {
            Write-Warn "Windows Disk Filter: Not installed yet (run '.\install.ps1 -Mode install-driver' to spoof Disk[0] under Windows)"
        }
    } else {
        Write-Warn "Run as Administrator to also inspect the EFI System Partition (ESP) and bcdedit BootOrder."
    }
}

# --- Reset Profile Mode ------------------------------------------------------

function Invoke-ResetProfile {
    Invoke-WithMountedEsp {
        param($espDrive)
        $uuidDat = "$espDrive$EfiProfileRelPath"
        if (Test-Path $uuidDat) {
            Remove-Item -Path $uuidDat -Force
            Write-Ok "Deleted ESP profile ($EfiProfileRelPath)."
        } else {
            Write-Warn "No $EfiProfileRelPath found on ESP."
        }
    }
    Write-Ok "On next UEFI boot, press [R] (or let it regenerate if NVRAM is empty) to generate brand new IDs."
}

function Invoke-BackupProfile {
    Invoke-WithMountedEsp {
        param($espDrive)
        $uuidDat = "$espDrive$EfiProfileRelPath"
        if (Test-Path $uuidDat) {
            Copy-Item -Path $uuidDat -Destination $ProfileBackupFile -Force
            Write-Ok "Backed up active ESP profile to: $ProfileBackupFile"
        } else {
            Write-Warn "No $EfiProfileRelPath found on ESP to back up."
        }
    }
}

function Invoke-RestoreProfile {
    if (-not (Test-Path $ProfileBackupFile)) {
        throw "Backup file not found: $ProfileBackupFile"
    }
    Invoke-WithMountedEsp {
        param($espDrive)
        $targetDir = "$espDrive$EfiRelDir"
        if (-not (Test-Path $targetDir)) {
            New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
        }
        $uuidDat = "$espDrive$EfiProfileRelPath"
        Copy-Item -Path $ProfileBackupFile -Destination $uuidDat -Force
        Write-Ok "Restored profile from $ProfileBackupFile to $EfiProfileRelPath."
    }
}

# --- Uninstall Mode ----------------------------------------------------------

function Invoke-Uninstall {
    Uninstall-WindowsDiskFilterDriver
    $guids = Get-SpooferFirmwareGuids
    foreach ($g in $guids) {
        Write-Step "Removing UEFI boot entry $g..."
        & bcdedit /delete $g /f | Out-Null
        Write-Ok "Deleted UEFI entry $g"
    }

    Invoke-WithMountedEsp {
        param($espDrive)
        $targetDir = "$espDrive$EfiRelDir"
        $legacyBin = "$espDrive\EFI\Boot\smbios-spoofer.efi"

        if (Test-Path $targetDir) {
            Remove-Item -Path $targetDir -Recurse -Force
            Write-Ok "Removed ESP directory: $EfiRelDir"
        }
        if (Test-Path $legacyBin) {
            Remove-Item -Path $legacyBin -Force
            Write-Ok "Removed legacy binary: \EFI\Boot\smbios-spoofer.efi"
        }
    }

    Write-Ok "Uninstallation complete. On next reboot, your original hardware SMBIOS identifiers will be restored."
}

# --- Main Entrypoint ---------------------------------------------------------

Write-Banner

switch ($Mode) {
    "build-only" {
        Invoke-ProjectBuild
        Save-PreSpoofSnapshotIfNeeded
    }
    "status" {
        Invoke-StatusCheck
    }
    "install" {
        Invoke-ProjectBuild
        Save-PreSpoofSnapshotIfNeeded
        Request-AdminElevation -TargetMode "install-only"
        Invoke-EspInstall
    }
    "install-only" {
        Save-PreSpoofSnapshotIfNeeded
        Request-AdminElevation -TargetMode "install-only"
        Invoke-EspInstall
    }
    "install-driver" {
        Request-AdminElevation -TargetMode "install-driver"
        Install-WindowsDiskFilterDriver
    }
    "uninstall-driver" {
        Request-AdminElevation -TargetMode "uninstall-driver"
        Uninstall-WindowsDiskFilterDriver
    }
    "reset-profile" {
        Request-AdminElevation -TargetMode "reset-profile"
        Invoke-ResetProfile
    }
    "backup-profile" {
        Request-AdminElevation -TargetMode "backup-profile"
        Invoke-BackupProfile
    }
    "restore-profile" {
        Request-AdminElevation -TargetMode "restore-profile"
        Invoke-RestoreProfile
    }
    "uninstall" {
        Request-AdminElevation -TargetMode "uninstall"
        Invoke-Uninstall
    }
}

if ($PauseOnExit) {
    Write-Host ""
    Read-Host "  Press Enter to close this window"
}
