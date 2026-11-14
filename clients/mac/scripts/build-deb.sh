#!/bin/sh
# Builds the app in release and packages it as a .deb for x86-64 Debian,
# Ubuntu and Zorin, in the folder given:
#
#   scripts/build-deb.sh <folder>
#
# It installs the app as /usr/bin/renoa, with a menu entry and the icon drawn
# from assets/icon.svg. RENOA_VERSION, when set, is the release the package
# is built as; without it the package carries Cargo.toml's version. The app
# does not update itself on Linux (src/update.rs): a newer .deb replaces it.
set -eu
out=${1:?usage: scripts/build-deb.sh <folder>}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$(dirname "$0")/.."

[ "$(uname -s)-$(uname -m)" = Linux-x86_64 ] || {
    echo "build-deb.sh builds on x86-64 Linux" >&2
    exit 1
}

cargo build --release
version=${RENOA_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
root="$work/renoa"
mkdir -p "$root/DEBIAN" "$root/usr/bin" "$root/usr/share/applications" \
    "$root/usr/share/icons/hicolor/scalable/apps"
install -m 755 target/release/renoa-mac "$root/usr/bin/renoa"
# The name matches the window's app_id (src/main.rs), so the desktop shows
# this icon for the open window.
install -m 644 assets/icon.svg "$root/usr/share/icons/hicolor/scalable/apps/live.renoa.Renoa.svg"

cat > "$root/usr/share/applications/live.renoa.Renoa.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Renoa
Comment=Chat with your agents
Exec=renoa
Icon=live.renoa.Renoa
StartupWMClass=live.renoa.Renoa
Categories=Network;Chat;
Terminal=false
DESKTOP

# The libraries the app links or opens: its display (Wayland or X11), its
# keyboard maps and Vulkan. Its key and sign-in are kept by GNOME's keyring
# through the Secret Service; KDE Wallet keeps only text, which the binary
# key is not.
cat > "$root/DEBIAN/control" <<CONTROL
Package: renoa
Version: $version
Architecture: amd64
Maintainer: Anrahya (https://github.com/Anrahya/Renoa)
Depends: libc6, libxcb1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libx11-xcb1, libvulkan1
Recommends: gnome-keyring, mesa-vulkan-drivers
Section: net
Priority: optional
Homepage: https://renoa.live
Description: Renoa for Linux
 A thin chat with your Renoa agents, signed in with a code from your phone.
CONTROL

# Whatever the umask made, only root may change what the package installs.
chmod -R go-w "$root"
deb="$out/renoa_${version}_amd64.deb"
dpkg-deb --root-owner-group --build "$root" "$deb" >/dev/null
echo "$deb"
