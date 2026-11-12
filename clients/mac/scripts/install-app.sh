#!/bin/sh
# Builds the app in release, wraps it as Renoa.app with its icon, signs it with
# the "Renoa Dev" identity when this Mac has one, and installs it in
# /Applications (or ~/Applications when /Applications is not writable).
#
# An installed app stays the same binary until the next install, so once the
# Keychain is told to Always Allow it, it stops asking for the login password.
# A rebuild run with `cargo run` is a new binary and asks again.
set -eu
cd "$(dirname "$0")/.."

cargo build --release
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$work/Renoa.app"
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
    echo "No \"Renoa Dev\" signing identity: the Keychain will ask again after each install (see README.md)." >&2
fi

if [ -w /Applications ]; then
    destination=/Applications
else
    destination="$HOME/Applications"
    mkdir -p "$destination"
fi
rm -rf "$destination/Renoa.app"
ditto "$app" "$destination/Renoa.app"
echo "Installed $destination/Renoa.app ($version)"
