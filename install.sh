#!/usr/bin/env bash
# ==============================================================================
#  install.sh — Build, test & install smbios-spoofer.efi (Windows & Linux UEFI)
# ==============================================================================
#  Usage:
#    ./install.sh                    # Build + install sur l'ESP + BootOrder auto
#    ./install.sh --headless         # Build en mode headless + install
#    ./install.sh --spoof-mac        # Build avec spoof MAC + install
#    ./install.sh --status           # Vérifier l'état SMBIOS actuel
#    ./install.sh --build-only       # Compile + tests uniquement
#    ./install.sh --install-only     # Installe le .efi déjà compilé
#    ./install.sh --uninstall        # Supprime l'entrée UEFI et nettoie l'ESP
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Sous Windows (Git Bash / MSYS2 / Cygwin), déléguer directement à install.ps1
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        PS_ARGS=("-NoProfile" "-ExecutionPolicy" "Bypass" "-File" "$SCRIPT_DIR/install.ps1")
        MODE="install"
        for arg in "$@"; do
            case "$arg" in
                --headless)      PS_ARGS+=("-Headless") ;;
                --spoof-mac)     PS_ARGS+=("-SpoofMac") ;;
                --skip-tests)    PS_ARGS+=("-SkipTests") ;;
                --status)        MODE="status" ;;
                --build-only)    MODE="build-only" ;;
                --install-only)  MODE="install-only" ;;
                --reset-profile) MODE="reset-profile" ;;
                --uninstall)     MODE="uninstall" ;;
                --help|-h)
                    echo "Usage: ./install.sh [--headless] [--spoof-mac] [--status|--build-only|--install-only|--reset-profile|--uninstall]"
                    exit 0
                    ;;
            esac
        done
        PS_ARGS+=("-Mode" "$MODE")
        exec powershell.exe "${PS_ARGS[@]}"
        ;;
esac

# ─── Environnement Linux / Unix ───────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { echo -e "${CYAN}[*]${NC} $1"; }
ok()    { echo -e "${GREEN}[+]${NC} $1"; }
warn()  { echo -e "${YELLOW}[!]${NC} $1"; }
err()   { echo -e "${RED}[-]${NC} $1"; exit 1; }

HEADLESS=false
SPOOF_MAC=false
MODE="install"
EFI_BINARY="target/x86_64-unknown-uefi/release/smbios-spoofer.efi"

for arg in "$@"; do
    case "$arg" in
        --headless)      HEADLESS=true ;;
        --spoof-mac)     SPOOF_MAC=true ;;
        --build-only)    MODE="build-only" ;;
        --install-only)  MODE="install-only" ;;
        --uninstall)     MODE="uninstall" ;;
        --status)        MODE="status" ;;
        --help|-h)
            echo "Usage: ./install.sh [--headless] [--spoof-mac] [--status|--build-only|--install-only|--uninstall]"
            exit 0
            ;;
        *) err "Option inconnue: $arg" ;;
    esac
done

do_build() {
    command -v cargo >/dev/null 2>&1 || err "Rust/cargo non trouvé. Installez via https://rustup.rs"
    if ! rustup target list --installed | grep -q "x86_64-unknown-uefi"; then
        info "Ajout de la cible x86_64-unknown-uefi..."
        rustup target add x86_64-unknown-uefi
    fi

    FEATURES=()
    $HEADLESS  && FEATURES+=("headless")
    $SPOOF_MAC && FEATURES+=("spoof-mac")

    BUILD_ARGS=("build" "--release" "--target" "x86_64-unknown-uefi")
    if [ ${#FEATURES[@]} -gt 0 ]; then
        FEAT_STR=$(IFS=,; echo "${FEATURES[*]}")
        BUILD_ARGS+=("--features" "$FEAT_STR")
        info "Compilation UEFI (features: $FEAT_STR)..."
    else
        info "Compilation UEFI (mode GOP standard)..."
    fi

    cargo "${BUILD_ARGS[@]}"
    cp -f "$EFI_BINARY" "$SCRIPT_DIR/smbios-spoofer.efi"
    ok "Compilé : $EFI_BINARY"
}

find_linux_esp() {
    for candidate in /boot/efi /efi /boot; do
        if [ -d "$candidate/EFI" ]; then
            echo "$candidate"
            return 0
        fi
    done
    err "Partition EFI non trouvée (/boot/efi ou /efi)."
}

do_install_linux() {
    [ -f "$EFI_BINARY" ] || err "Binaire $EFI_BINARY introuvable."
    ESP_MNT="$(find_linux_esp)"
    sudo mkdir -p "$ESP_MNT/EFI/SmbiosSpoofer"
    sudo cp -f "$EFI_BINARY" "$ESP_MNT/EFI/SmbiosSpoofer/smbios-spoofer.efi"
    ok "Copié vers $ESP_MNT/EFI/SmbiosSpoofer/smbios-spoofer.efi"

    if command -v efibootmgr >/dev/null 2>&1; then
        ESP_DEV=$(df "$ESP_MNT" | tail -1 | awk '{print $1}')
        DISK_DEV=$(lsblk -no pkname "$ESP_DEV" 2>/dev/null | head -1)
        PART_NUM=$(cat "/sys/class/block/$(basename "$ESP_DEV")/partition" 2>/dev/null || echo "1")
        if [ -n "$DISK_DEV" ]; then
            info "Enregistrement de l'entrée UEFI via efibootmgr sur /dev/$DISK_DEV (part $PART_NUM)..."
            sudo efibootmgr -c -d "/dev/$DISK_DEV" -p "$PART_NUM" -L "EFI SMBIOS Spoofer" -l '\EFI\SmbiosSpoofer\smbios-spoofer.efi' || warn "efibootmgr a retourné un avertissement"
            ok "Entrée UEFI enregistrée via efibootmgr"
        fi
    else
        warn "efibootmgr non installé : ajoutez manuellement \\EFI\\SmbiosSpoofer\\smbios-spoofer.efi dans votre BIOS."
    fi
}

case "$MODE" in
    build-only)
        do_build
        ;;
    install)
        do_build
        do_install_linux
        ;;
    install-only)
        do_install_linux
        ;;
    status)
        sudo dmidecode -t system -t baseboard -t bios -t memory 2>/dev/null || warn "Exécutez avec sudo pour lire dmidecode"
        ;;
    uninstall)
        ESP_MNT="$(find_linux_esp)"
        sudo rm -rf "$ESP_MNT/EFI/SmbiosSpoofer"
        ok "Supprimé : $ESP_MNT/EFI/SmbiosSpoofer"
        ;;
esac
