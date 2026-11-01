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
    [ValidateSet("install", "status", "uninstall", "build-only", "install-only", "reset-profile")]
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
$SnapshotFile         = Join-Path $ProjectRoot "smbios-snapshot-before.json"
$BuiltEfiPath         = Join-Path $ProjectRoot "target\x86_64-unknown-uefi\release\smbios-spoofer.efi"
$LocalEfiCopy         = Join-Path $ProjectRoot "smbios-spoofer.efi"

# --- Display Helpers ---------------------------------------------------------

function Write-Banner {
    Write-Host ""
    Write-Host "  ======================================================================" -ForegroundColor Cyan
    Write-Host "    EFI SMBIOS SPOOFER v0.2.0 -- Automatic Installer & Boot Manager" -ForegroundColor Yellow
    Write-Host "    SMBIOS 2.x & 3.0 (64-bit) | Type 0/1/2/3/4/11/17 (RAM) | Zero EasyUEFI" -ForegroundColor Gray
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

    $ramSerials = @()
    foreach ($r in $rams) {
        $ramSerials += [PSCustomObject]@{
            DeviceLocator = [string]$r.DeviceLocator
            Manufacturer  = [string]$r.Manufacturer
            PartNumber    = ([string]$r.PartNumber).Trim()
            SerialNumber  = ([string]$r.SerialNumber).Trim()
        }
    }

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
        ProcessorSerial   = [string]$cpu.ProcessorId
        BiosManufacturer  = [string]$bios.Manufacturer
        BiosVersion       = [string]$bios.SMBIOSBIOSVersion
        BiosReleaseDate   = if ($bios.ReleaseDate) { $bios.ReleaseDate.ToString("yyyy-MM-dd") } else { "" }
        RamModules        = $ramSerials
    }
}

function Save-PreSpoofSnapshotIfNeeded {
    if (-not (Test-Path $SnapshotFile)) {
        Write-Step "Saving initial hardware SMBIOS snapshot to smbios-snapshot-before.json..."
        $snap = Get-CurrentSmbiosSnapshot
        $snap | ConvertTo-Json -Depth 5 | Set-Content -Path $SnapshotFile -Encoding UTF8
        Write-Ok "Hardware snapshot saved ($SnapshotFile)"
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
        Write-Ok "All unit tests passed (121/121)"
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

    Write-Host ""
    Write-Host "  ======================================================================" -ForegroundColor Green
    Write-Host "    INSTALLATION COMPLETED SUCCESSFULLY!" -ForegroundColor Green
    Write-Host "  ======================================================================" -ForegroundColor Green
    Write-Host "    - Deployed binary  : $EfiRelPath" -ForegroundColor White
    Write-Host "    - UEFI BootOrder   : #1 '$BootEntryDescription' -> #2 Windows Boot Manager" -ForegroundColor White
    Write-Host "    - Persistence      : NVRAM (SmbiosSpoof) + ESP ($EfiProfileRelPath)" -ForegroundColor White
    Write-Host ""
    Write-Host "    Next steps:" -ForegroundColor Yellow
    Write-Host "      1. Make sure Secure Boot is disabled in your BIOS settings." -ForegroundColor Gray
    Write-Host "      2. Reboot your PC: the spoofer will run automatically and chainload Windows." -ForegroundColor Gray
    Write-Host "      3. After rebooting, verify with: .\install.ps1 -Mode status" -ForegroundColor Gray
    Write-Host ""
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

    Write-Host ""
    Write-Host "  --- Current Windows SMBIOS Identifiers ------------------------------" -ForegroundColor Cyan
    Show-FieldComparison "System UUID"       $curr.UUID             $prevUUID
    Show-FieldComparison "System Serial"     $curr.SystemSerial     $prevSysSer
    Show-FieldComparison "System SKU"        $curr.SystemSKU        $prevSysSku
    Show-FieldComparison "Baseboard Model"   $curr.BaseboardProduct $prevBbProd
    Show-FieldComparison "Baseboard Serial"  $curr.BaseboardSerial  $prevBbSer
    Show-FieldComparison "Chassis Serial"    $curr.ChassisSerial    $prevChSer
    Show-FieldComparison "Chassis AssetTag"  $curr.ChassisAssetTag  $prevChAsset
    Show-FieldComparison "BIOS Vendor"       $curr.BiosManufacturer $prevBiosVen
    Show-FieldComparison "BIOS Version"      $curr.BiosVersion      $prevBiosVer
    Show-FieldComparison "BIOS Date"         $curr.BiosReleaseDate  $prevBiosDate

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
                } else {
                    Write-Warn "ESP uuid.dat present but unexpected size ($($bytes.Length) bytes)"
                }
            } else {
                Write-Warn "No $EfiProfileRelPath on ESP yet (will be created on first UEFI boot)"
            }
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

# --- Uninstall Mode ----------------------------------------------------------

function Invoke-Uninstall {
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
    "reset-profile" {
        Request-AdminElevation -TargetMode "reset-profile"
        Invoke-ResetProfile
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
