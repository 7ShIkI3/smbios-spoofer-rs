#!/usr/bin/env bash
set -euo pipefail

# Build script pour le SMBIOS Spoofer en Rust
# Compile pour la cible UEFI x86_64

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "=== SMBIOS Spoofer (Rust) — Build ==="
echo ""

# Vérifier que la cible UEFI est installée
if ! rustup target list --installed | grep -q "x86_64-unknown-uefi"; then
    echo "Installation de la cible x86_64-unknown-uefi..."
    rustup target add x86_64-unknown-uefi
fi

# Installer les composants nécessaires pour build-std
if ! rustup component list --installed | grep -q "rust-src"; then
    echo "Installation du composant rust-src (nécessaire pour build-std)..."
    rustup component add rust-src
fi

echo "Compilation en mode release..."
echo ""

cargo build --release

echo ""
echo "=== Build terminé ==="
echo "Le fichier .efi se trouve dans :"
echo "  target/x86_64-unknown-uefi/release/smbios-spoofer.efi"
echo ""
echo "Pour installer :"
echo "  1. Copier smbios-spoofer.efi vers la partition EFI"
echo "  2. Désactiver Secure Boot dans le BIOS"
echo "  3. Ajouter une entrée de boot UEFI (via EasyUEFI ou bcfg)"
echo ""
echo "Taille du binaire :"
ls -lh target/x86_64-unknown-uefi/release/smbios-spoofer.efi 2>/dev/null || echo "  (binaire non trouvé)"
