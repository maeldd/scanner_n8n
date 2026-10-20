#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
TARGET="${TARGET:-x86_64-pc-windows-gnu}"
EXE_REL="target/$TARGET/release/scanner_n8n.exe"

echo "==> Compilation Windows ($TARGET, version $VERSION)"
cargo build --release --locked --target "$TARGET"

if ! command -v makensis >/dev/null 2>&1; then
    echo "" >&2
    echo "makensis (NSIS) est requis pour créer l'installateur." >&2
    echo "  macOS          : brew install nsis" >&2
    echo "  Debian/Ubuntu  : sudo apt install nsis" >&2
    echo "" >&2
    echo "L'exécutable Windows a toutefois été compilé :" >&2
    echo "  $ROOT/$EXE_REL" >&2
    exit 1
fi

# NSIS attend un chemin natif sous Windows (Git Bash/MSYS).
if command -v cygpath >/dev/null 2>&1; then
    ROOT_NATIVE="$(cygpath -m "$ROOT")"
    EXE_PATH="$(cygpath -m "$ROOT/$EXE_REL")"
else
    ROOT_NATIVE="$ROOT"
    EXE_PATH="$ROOT/$EXE_REL"
fi

mkdir -p dist
cp "$EXE_REL" "dist/Scanner-n8n-$VERSION-x64.exe"

echo "==> Création de l'installateur"
makensis -V2 \
    -DROOT="$ROOT_NATIVE" \
    -DVERSION="$VERSION" \
    -DEXE_PATH="$EXE_PATH" \
    packaging/windows/installer.nsi

echo ""
echo "==> Terminé :"
echo "    dist/Scanner-n8n-$VERSION-x64.exe"
echo "    dist/Scanner-n8n-Setup-$VERSION-x64.exe"
