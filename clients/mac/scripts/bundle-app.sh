#!/bin/sh
# Builds the app in release and wraps it as Renoa.app, with its icon, in the
# folder given, signed with the "Renoa Dev" identity when this Mac has one.
#
#   scripts/bundle-app.sh <folder>
#
# RENOA_VERSION, when set, is the release the build is published as: the app
# shows it and updates itself from renoa.live (src/update.rs). Without it the
# app carries Cargo.toml's version and never updates. install-app.sh and
# scripts/publish-mac-app run this.
set -eu
out=${1:?usage: scripts/bundle-app.sh <folder>}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$(dirname "$0")/.."

cargo build --release
version=${RENOA_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$out/Renoa.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/renoa-mac "$app/Contents/MacOS/renoa-mac"

# The icon: assets/icon.svg drawn at every size macOS asks for.
iconset="$work/Renoa.iconset"
mkdir -p "$iconset"
sips -s format png assets/icon.svg --out "$work/icon.png" >/dev/null
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$work/icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$work/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Renoa.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Renoa</string>
    <key>CFBundleDisplayName</key><string>Renoa</string>
    <key>CFBundleIdentifier</key><string>live.renoa.mac</string>
    <key>CFBundleExecutable</key><string>renoa-mac</string>
    <key>CFBundleIconFile</key><string>Renoa</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

if security find-identity -p codesigning | grep -qi '"Renoa Dev"'; then
    identity=$(security find-identity -p codesigning | sed -n 's/.*"\([Rr]enoa [Dd]ev\)".*/\1/p' | head -n 1)
    codesign --force --sign "$identity" --identifier live.renoa.mac "$app"
else
    echo "No \"Renoa Dev\" signing identity: the Keychain will ask again after each install, and the app can't update itself (see README.md)." >&2
fi
