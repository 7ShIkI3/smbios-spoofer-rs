#!/usr/bin/env bash
set -euo pipefail
#
# sign.sh — Signe un binaire .efi pour Secure Boot
#
# Ce script :
# 1. Génère une MOK (Machine Owner Key) — ta clé perso
# 2. Signe le .efi avec cette clé via sbsign
# 3. Affiche les instructions pour enrôler la MOK dans le firmware
#
# Prérequis : sudo apt install sbsigntool efitools  (Debian/Ubuntu)
#             ou équivalent sur ta distro
#
# DISCLAIMER : ceci n'est PAS un "bypass" de Secure Boot.
# C'est le mécanisme STANDARD pour exécuter un binaire EFI
# personnalisé sans désactiver Secure Boot.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KEYS_DIR="$SCRIPT_DIR/keys"

EFI_BINARY="${1:-$SCRIPT_DIR/target/x86_64-unknown-uefi/release/smbios-spoofer.efi}"
SIGNED_BINARY="${EFI_BINARY%.efi}-signed.efi"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

echo -e "${CYAN}=== EFI Binary Signing Tool ===${NC}"
echo ""

# ─── Vérifier les dépendances ────────────────────────────────────────

check_tool() {
    if ! command -v "$1" &>/dev/null; then
        echo -e "${RED}ERREUR: $1 n'est pas installé.${NC}"
        echo "Installe-le via : sudo apt install $2 (Debian/Ubuntu)"
        exit 1
    fi
}

check_tool openssl openssl
check_tool sbsign sbsigntool

# ─── Vérifier le binaire ─────────────────────────────────────────────

if [[ ! -f "$EFI_BINARY" ]]; then
    echo -e "${RED}ERREUR: binaire $EFI_BINARY introuvable.${NC}"
    echo "Compile d'abord avec : ./build.sh"
    echo "Ou passe le chemin en argument : $0 /chemin/vers/mon.efi"
    exit 1
fi

echo -e "Binaire à signer : ${GREEN}$EFI_BINARY${NC}"
echo ""

# ─── Générer les clés (si pas déjà fait) ─────────────────────────────

mkdir -p "$KEYS_DIR"

if [[ ! -f "$KEYS_DIR/MOK.key" ]] || [[ ! -f "$KEYS_DIR/MOK.crt" ]]; then
    echo -e "${YELLOW}[1/4] Génération de la Machine Owner Key (MOK)...${NC}"
    echo ""

    # Génère la clé privée RSA 2048 bits
    openssl req -new -x509 -newkey rsa:2048 \
        -subj "/CN=SMBIOS Spoofer MOK/" \
        -keyout "$KEYS_DIR/MOK.key" \
        -out "$KEYS_DIR/MOK.crt" \
        -days 36500 \
        -nodes \
        -sha256

    # Convertit le certificat en format DER (requis par le firmware)
    openssl x509 -in "$KEYS_DIR/MOK.crt" \
        -outform DER -out "$KEYS_DIR/MOK.der"

    echo ""
    echo -e "${GREEN}Clés générées dans $KEYS_DIR/ :${NC}"
    echo "  MOK.key  → ta clé privée (GARDE-LA PRÉCIEUSEMENT)"
    echo "  MOK.crt  → ton certificat (PEM)"
    echo "  MOK.der  → ton certificat (DER, pour le firmware)"
    echo ""
else
    echo -e "${YELLOW}[1/4] Clés existantes trouvées, réutilisation.${NC}"
    echo ""
fi

# ─── Signer le binaire ───────────────────────────────────────────────

echo -e "${YELLOW}[2/4] Signature du binaire EFI...${NC}"

sbsign \
    --key "$KEYS_DIR/MOK.key" \
    --cert "$KEYS_DIR/MOK.crt" \
    --output "$SIGNED_BINARY" \
    "$EFI_BINARY"

echo -e "${GREEN}Binaire signé : $SIGNED_BINARY${NC}"
echo ""

# ─── Vérifier la signature ───────────────────────────────────────────

echo -e "${YELLOW}[3/4] Vérification de la signature...${NC}"

if sbverify --cert "$KEYS_DIR/MOK.crt" "$SIGNED_BINARY" 2>&1; then
    echo -e "${GREEN}✓ Signature valide${NC}"
else
    echo -e "${RED}✗ Échec de vérification${NC}"
    exit 1
fi
echo ""

# ─── Instructions d'enrôlement ───────────────────────────────────────

echo -e "${YELLOW}[4/4] Instructions d'enrôlement MOK${NC}"
echo ""

cat << EOF
┌─────────────────────────────────────────────────────────────────────────────┐
│                     ENRÔLEMENT DE LA MOK — À FAIRE                          │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Tu dois enrôler ta clé dans le firmware pour que Secure Boot l'accepte.    │
│  Copie le fichier ${CYAN}keys/MOK.der${NC} sur une clé USB FAT32.                 │
│                                                                             │
│  MÉTHODE 1 : Via le BIOS ASUS TUF                                           │
│  ─────────────────────────────                                              │
│  1. Redémarre et appuie sur F2/DEL pour entrer dans le BIOS                 │
│  2. Cherche "Secure Boot" → "Key Management"                                │
│  3. Cherche "Append Key" ou "Enroll Key" ou "Install Factory Defaults"     │
│     → Si tu vois "Enroll EFI Image" : tu peux enrôler le .efi signé        │
│        directement (pas besoin du .der séparé)                              │
│  4. Sélectionne le fichier .der (ou .efi signé) sur ta clé USB              │
│  5. Redémarre                                                                │
│                                                                             │
│  MÉTHODE 2 : Via mokutil (si Linux est installé)                            │
│  ───────────────────────────────────────────                                │
│  1. sudo mokutil --import keys/MOK.der                                       │
│  2. Redémarre                                                                │
│  3. Le MOK Manager (écran bleu) apparaîtra automatiquement                  │
│  4. Choisis "Enroll MOK" → "Continue" → "Yes" → "Reboot"                    │
│                                                                             │
│  ⚠️  Si aucune de ces options n'apparaît dans ton BIOS ASUS :                │
│     Ton firmware est verrouillé (pas de custom key support).                 │
│     → Tu DOIS désactiver Secure Boot. Il n'y a pas d'alternative.           │
│     → C'est le cas pour la plupart des laptops gaming grand public.         │
│                                                                             │
│  Après enrôlement, copie le .efi signé sur ta partition EFI :               │
│    cp ${SIGNED_BINARY} Z:\\\\EFI\\\\SmbiosSpoofer\\\\SmbiosSpooferV3.efi         │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
EOF

echo ""
echo -e "${CYAN}Résumé des fichiers générés :${NC}"
ls -lh "$KEYS_DIR/"
echo ""
ls -lh "$SIGNED_BINARY"
