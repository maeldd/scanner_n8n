#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_NAME="Scanner n8n"
APP_EXE="scanner_n8n"
BUNDLE_ID="${BUNDLE_ID:-cloud.dronelis.scanner-n8n}"
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/Scanner-n8n-$VERSION-universal.dmg"
BARE_BIN="$DIST/Scanner-n8n-$VERSION-universal"

ARM64_TARGET="aarch64-apple-darwin"
X86_64_TARGET="x86_64-apple-darwin"
# macOS 11 est le minimum pour les processeurs Apple Silicon.
ARM64_MIN="11.0"
# winit/eframe sont testés à partir de macOS 10.14. Cette cible permet de
# faire tourner l'application sur les Mac Intel encore sous Mojave ou plus récent.
X86_64_MIN="10.14"

for target in "$ARM64_TARGET" "$X86_64_TARGET"; do
    if ! rustup target list --installed | grep -qx "$target"; then
        echo "Cible Rust manquante : $target" >&2
        echo "Installez-la avec : rustup target add $target" >&2
        exit 1
    fi
done

echo "==> Compilation macOS universelle (arm64 + x86_64, version $VERSION)"
MACOSX_DEPLOYMENT_TARGET="$ARM64_MIN" \
    cargo build --release --locked --target "$ARM64_TARGET"
MACOSX_DEPLOYMENT_TARGET="$X86_64_MIN" \
    cargo build --release --locked --target "$X86_64_TARGET"

ARM64_BIN="target/$ARM64_TARGET/release/$APP_EXE"
X86_64_BIN="target/$X86_64_TARGET/release/$APP_EXE"
UNIVERSAL_BIN="target/release/$APP_EXE-universal"

echo "==> Assemblage du binaire universel"
lipo -create -output "$UNIVERSAL_BIN" "$ARM64_BIN" "$X86_64_BIN"
lipo -info "$UNIVERSAL_BIN"

echo "==> Création du bundle $APP_NAME.app"
mkdir -p "$DIST"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$UNIVERSAL_BIN" "$APP/Contents/MacOS/$APP_EXE"
cp assets/icon-dark.icns "$APP/Contents/Resources/AppIcon.icns"

# Binaire nu, pour ceux qui ne veulent pas du bundle.
cp "$UNIVERSAL_BIN" "$BARE_BIN"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleExecutable</key>
    <string>$APP_EXE</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleDevelopmentRegion</key>
    <string>fr</string>
    <key>LSMinimumSystemVersion</key>
    <string>$X86_64_MIN</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>NSHumanReadableCopyright</key>
    <string>MIT License</string>
</dict>
</plist>
PLIST

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    echo "==> Signature avec l'identité : $APPLE_SIGNING_IDENTITY"
    codesign --force --options runtime --timestamp \
        --sign "$APPLE_SIGNING_IDENTITY" "$APP"
    codesign --force --options runtime --timestamp \
        --sign "$APPLE_SIGNING_IDENTITY" "$BARE_BIN"
else
    echo "==> Signature ad-hoc (ouverture locale uniquement, non notariée)"
    codesign --force --sign - "$APP"
    codesign --force --sign - "$BARE_BIN"
fi

codesign --verify --strict "$APP"
echo "    Signature vérifiée"

echo "==> Création du DMG"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/scanner-n8n-dmg.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT

cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

rm -f "$DMG"
hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$STAGE" \
    -ov \
    -format UDZO \
    "$DMG" >/dev/null

echo ""
echo "==> Terminé :"
echo "    $APP"
echo "    $DMG"
echo "    $BARE_BIN"
