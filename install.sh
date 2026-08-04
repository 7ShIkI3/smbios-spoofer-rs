#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
#  SMBIOS Spoofer — Installateur Windows
# =============================================================================
#
#  Ce script :
#    1. Vérifie/installe la toolchain Rust + cible UEFI
#    2. Compile le projet en mode release
#    3. Monte la partition EFI Windows (nécessite admin)
#    4. Copie le .efi sur la partition EFI
#    5. Affiche les instructions pour ajouter l'entrée de boot
#
#  Usage :
#    ./install.sh              → build + install interactif
#    ./install.sh --build-only → juste compiler
#    ./install.sh --install    → juste copier (si déjà compilé)
# =============================================================================

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EFI_FILE="$SCRIPT_DIR/target/x86_64-unknown-uefi/release/smbios-spoofer.efi"
EFI_DEST_DIR="\\EFI\\SmbiosSpoofer"
EFI_DEST_FILE="$EFI_DEST_DIR\\SmbiosSpooferV3.efi"

MODE="${1:---full}"

# ─── Bannière ────────────────────────────────────────────────────────────

echo ""
echo -e "${CYAN}+==============================================================+${NC}"
echo -e "${CYAN}|${NC}              ${BOLD}SMBIOS Spoofer — Installateur${NC}                   ${CYAN}|${NC}"
echo -e "${CYAN}|${NC}              Type 1/2/3/4 — Full SMBIOS Spoof              ${CYAN}|${NC}"
echo -e "${CYAN}+==============================================================+${NC}"
echo ""

# ─── Vérification admin (pour montage EFI) ──────────────────────────────

check_admin() {
    if [[ "$MODE" == "--build-only" ]]; then
        return 0
    fi
    # Vérifie si on peut accéder à un dossier protégé (test admin Windows)
    if ! cmd.exe /c "net session" &>/dev/null 2>&1; then
        echo -e "${YELLOW}[!] Ce script nécessite des droits administrateur pour monter${NC}"
        echo -e "${YELLOW}    la partition EFI. Relance avec :${NC}"
        echo -e "${YELLOW}    → Clic droit sur Git Bash → 'Run as administrator'${NC}"
        echo ""
        echo -e "${CYAN}    Ou utilise --build-only pour juste compiler :${NC}"
        echo -e "${CYAN}    ./install.sh --build-only${NC}"
        echo ""
        if [[ "$MODE" == "--full" ]]; then
            echo -e "${YELLOW}    Passage en mode build-only...${NC}"
            MODE="--build-only"
        fi
    fi
}

# ─── Étape 1 : Vérification Rust ────────────────────────────────────────

check_rust() {
    echo -e "${BOLD}[1/5] Vérification de la toolchain Rust...${NC}"

    if ! command -v rustup &>/dev/null; then
        echo -e "${RED}[ERREUR] rustup n'est pas installé.${NC}"
        echo ""
        echo "  Télécharge et installe Rust depuis :"
        echo "  https://rustup.rs/"
        echo ""
        echo "  Puis relance ce script."
        exit 1
    fi

    echo -e "  $(rustup --version)"
    echo -e "  $(rustc --version)"

    if ! rustup target list --installed 2>/dev/null | grep -q "x86_64-unknown-uefi"; then
        echo -e "  ${YELLOW}Installation de la cible x86_64-unknown-uefi...${NC}"
        rustup target add x86_64-unknown-uefi
    fi
    echo -e "  ${GREEN}✓ cible x86_64-unknown-uefi installée${NC}"

    if ! rustup component list --installed 2>/dev/null | grep -q "rust-src"; then
        echo -e "  ${YELLOW}Installation de rust-src (build-std)...${NC}"
        rustup component add rust-src
    fi
    echo -e "  ${GREEN}✓ rust-src installé${NC}"

    echo ""
}

# ─── Étape 2 : Compilation ──────────────────────────────────────────────

build_project() {
    echo -e "${BOLD}[2/5] Compilation du projet...${NC}"
    cd "$SCRIPT_DIR"

    if cargo build --release --target x86_64-unknown-uefi 2>&1; then
        echo ""
        echo -e "  ${GREEN}✓ Compilation réussie${NC}"
    else
        echo ""
        echo -e "${RED}[ERREUR] La compilation a échoué.${NC}"
        exit 1
    fi

    if [[ ! -f "$EFI_FILE" ]]; then
        echo -e "${RED}[ERREUR] Le binaire .efi n'a pas été généré.${NC}"
        exit 1
    fi

    local size
    size=$(du -h "$EFI_FILE" | cut -f1)
    echo -e "  Binaire : ${GREEN}$EFI_FILE${NC} (${size})"
    echo ""
}

# ─── Étape 3 : Vérification Secure Boot ─────────────────────────────────

check_secure_boot() {
    echo -e "${BOLD}[3/5] Vérification Secure Boot...${NC}"

    # Vérifier si Secure Boot est actif via PowerShell
    local sb_status
    sb_status=$(powershell.exe -NoProfile -Command "Confirm-SecureBootUEFI" 2>/dev/null || echo "unknown")

    if [[ "$sb_status" == "True" ]] || [[ "$sb_status" == "true" ]]; then
        echo -e "  ${YELLOW}⚠ Secure Boot est ACTIVÉ.${NC}"
        echo ""
        echo -e "  ${YELLOW}Deux options :${NC}"
        echo -e "  ${YELLOW}  → Désactiver Secure Boot dans le BIOS${NC}"
        echo -e "  ${YELLOW}  → Signer le binaire avec ./sign.sh${NC}"
        echo ""
        echo -e "  ${CYAN}Pour l'instant, on continue sans signer.${NC}"
        echo -e "  ${CYAN}Lance ./sign.sh après l'installation si besoin.${NC}"
    else
        echo -e "  ${GREEN}✓ Secure Boot désactivé ou non supporté${NC}"
    fi
    echo ""
}

# ─── Étape 4 : Montage partition EFI + copie ────────────────────────────

install_efi() {
    echo -e "${BOLD}[4/5] Installation sur la partition EFI...${NC}"

    # --- Monter la partition EFI ---
    # Sur Windows, la partition EFI est cachée. On utilise mountvol pour la monter.
    echo ""
    echo -e "  ${YELLOW}Montage de la partition EFI...${NC}"
    echo -e "  ${YELLOW}(mountvol.exe nécessite des droits admin)${NC}"

    # Chercher un point de montage libre entre X: et Z:
    local drive_letter="S"

    # Démonter d'abord S: s'il est déjà utilisé (au cas où)
    mountvol.exe "S:" /D &>/dev/null || true &>/dev/null

    # Monter la partition EFI sur S:
    if ! mountvol.exe "S:" /S 2>/dev/null; then
        echo ""
        echo -e "  ${RED}[ERREUR] Impossible de monter la partition EFI.${NC}"
        echo -e "  ${RED}Vérifie que tu exécutes ce script en tant qu'administrateur.${NC}"
        echo ""
        echo -e "  ${CYAN}Alternative manuelle :${NC}"
        echo -e "  ${CYAN}  1. Ouvre cmd.exe en admin${NC}"
        echo -e "  ${CYAN}  2. mountvol S: /S${NC}"
        echo -e "  ${CYAN}  3. copy \"$EFI_FILE\" \"S:${EFI_DEST_FILE}\"${NC}"
        echo -e "  ${CYAN}  4. mountvol S: /D${NC}"
        return 1
    fi

    echo -e "  ${GREEN}✓ Partition EFI montée sur S:${NC}"

    # --- Créer le dossier de destination ---
    echo ""
    echo -e "  Création du dossier S:${EFI_DEST_DIR}..."

    if [[ ! -d "S:${EFI_DEST_DIR}" ]]; then
        mkdir -p "S:${EFI_DEST_DIR}" 2>/dev/null || {
            # mkdir peut échouer sur le format Windows, utiliser cmd.exe
            cmd.exe /c "mkdir S:${EFI_DEST_DIR}" 2>/dev/null || true
        }
    fi

    # --- Copier le .efi ---
    echo -e "  Copie de smbios-spoofer.efi → S:${EFI_DEST_FILE}..."
    cp "$EFI_FILE" "S:${EFI_DEST_FILE}" 2>/dev/null || {
        cmd.exe /c "copy \"$EFI_FILE\" \"S:${EFI_DEST_FILE}\" /Y" 2>/dev/null
    }

    if [[ -f "S:${EFI_DEST_FILE}" ]]; then
        echo -e "  ${GREEN}✓ Fichier copié avec succès${NC}"
    else
        echo -e "  ${RED}[ERREUR] La copie a échoué. Vérifie les permissions.${NC}"
        mountvol.exe "S:" /D &>/dev/null || true
        return 1
    fi

    echo ""

    # --- Démonter la partition EFI ---
    echo -e "  Démontage de la partition EFI..."
    mountvol.exe "S:" /D &>/dev/null || true
    echo -e "  ${GREEN}✓ Partition EFI démontée${NC}"
    echo ""
}

# ─── Étape 5 : Configuration boot ───────────────────────────────────────

boot_instructions() {
    echo -e "${BOLD}[5/5] Configuration de l'entrée de boot UEFI${NC}"
    echo ""

    cat << 'EOF'
  ╔══════════════════════════════════════════════════════════════════════╗
  ║  L'entrée de boot ne peut PAS être créée automatiquement.           ║
  ║  Tu dois utiliser l'OUTIL GRAPHIQUE EasyUEFI :                      ║
  ║                                                                      ║
  ║  MÉTHODE RECOMMANDÉE — EasyUEFI (gratuit)                           ║
  ║  ────────────────────────────────────────                            ║
  ║  1. Télécharge EasyUEFI depuis easyuefi.com                         ║
  ║  2. Lance EasyUEFI → onglet "Manage EFI Boot Option"                ║
  ║  3. Clique "Create New Entry"                                        ║
  ║     • Type : Linux or other OS                                       ║
  ║     • Name : SMBIOS Spoofer                                          ║
  ║     • File : \EFI\SmbiosSpoofer\SmbiosSpooferV3.efi                 ║
  ║  4. Monte cette entrée EN HAUT de la liste (1ère position)          ║
  ║  5. Redémarre                                                        ║
  ║                                                                      ║
  ║  MÉTHODE ALTERNATIVE — UEFI Shell                                    ║
  ║  ─────────────────────────────────                                   ║
  ║  1. Boot sur un shell UEFI (clé USB avec EFI Shell)                  ║
  ║  2. Tape :                                                           ║
  ║     bcfg boot dump                                                   ║
  ║     bcfg boot add 0 fs0:\EFI\SmbiosSpoofer\SmbiosSpooferV3.efi \    ║
  ║       "SMBIOS Spoofer"                                               ║
  ║  3. Redémarre                                                        ║
  ║                                                                      ║
  ║  ⚠️  Si Secure Boot est activé, le .efi DOIT être signé.            ║
  ║     Utilise ./sign.sh AVANT de créer l'entrée de boot.              ║
  ║     Puis utilise le .efi SIGNÉ dans l'entrée de boot.               ║
  ╚══════════════════════════════════════════════════════════════════════╝
EOF

    echo ""
    echo -e "  ${GREEN}Le spoofer se lancera AVANT Windows à chaque boot.${NC}"
    echo -e "  ${CYAN}Ordre de boot :${NC}"
    echo -e "  ${CYAN}  1. SMBIOS Spoofer → patch les tables → continue le boot${NC}"
    echo -e "  ${CYAN}  2. Windows Boot Manager → démarre Windows${NC}"
    echo ""
}

# ─── Résumé ─────────────────────────────────────────────────────────────

print_summary() {
    echo -e "${CYAN}+==============================================================+${NC}"
    echo -e "${CYAN}|${NC}              ${BOLD}Installation terminée !${NC}                         ${CYAN}|${NC}"
    echo -e "${CYAN}+==============================================================+${NC}"
    echo ""
    echo -e "  Fichier installé : ${GREEN}${EFI_DEST_FILE}${NC}"
    echo -e "  Types spoofés    : ${GREEN}1 (System) + 2 (Baseboard) + 3 (Chassis) + 4 (CPU)${NC}"
    echo ""
    echo -e "  ${BOLD}Prochaines étapes :${NC}"
    echo -e "  1. Créer l'entrée de boot avec EasyUEFI (voir instructions ci-dessus)"
    echo -e "  2. Si Secure Boot actif → lancer ${CYAN}./sign.sh${NC} et recopier le .efi signé"
    echo -e "  3. Redémarrer — le spoofer s'exécutera avant Windows"
    echo ""
}

# ─── MAIN ───────────────────────────────────────────────────────────────

check_admin
check_rust
build_project

if [[ "$MODE" == "--build-only" ]]; then
    echo -e "${GREEN}Build terminé. Le .efi est dans target/x86_64-unknown-uefi/release/${NC}"
    echo -e "${CYAN}Pour installer : relance avec ./install.sh --install${NC}"
    exit 0
fi

check_secure_boot
install_efi || {
    echo -e "${RED}L'installation EFI a échoué. Le build est quand même dispo dans target/.${NC}"
    exit 1
}
boot_instructions
print_summary
