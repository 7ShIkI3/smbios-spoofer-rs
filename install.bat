@echo off
setlocal EnableDelayedExpansion
title EFI SMBIOS Spoofer v0.2.0 - Automatic Installer

:: Se placer dans le dossier du script
cd /d "%~dp0"

:: Si un argument est passe (ex: install.bat -Mode status), le transmettre directement a install.ps1
if not "%~1"=="" (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
    exit /b !ERRORLEVEL!
)

echo.
echo   ======================================================================
echo     EFI SMBIOS SPOOFER v0.2.0 -- Menu d'Installation Automatique
echo   ======================================================================
echo.
echo     [1] Installer completement (Build + ESP + BootOrder UEFI auto)
echo     [2] Installer avec Spoof MAC actif (--features spoof-mac)
echo     [3] Installer en mode furtif Headless (--features headless,spoof-mac)
echo     [4] Verifier le statut actuel / Comparer les IDs SMBIOS (Status)
echo     [5] Compiler uniquement le binaire .efi (Build-Only)
echo     [6] Reinitialiser le profil de spoof (Reset Profile)
echo     [7] Desinstaller completement (Uninstall)
echo.
set /p CHOICE="  Votre choix [1-7, Entree=1] : "
if "%CHOICE%"=="" set CHOICE=1

if "%CHOICE%"=="1" (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Mode install
) else if "%CHOICE%"=="2" (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Mode install -SpoofMac
) else if "%CHOICE%"=="3" (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Mode install -Headless -SpoofMac
) else if "%CHOICE%"=="4" (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Mode status
) else if "%CHOICE%"=="5" (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Mode build-only
) else if "%CHOICE%"=="6" (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Mode reset-profile
) else if "%CHOICE%"=="7" (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Mode uninstall
) else (
    echo   [!] Choix invalide.
)

echo.
pause
