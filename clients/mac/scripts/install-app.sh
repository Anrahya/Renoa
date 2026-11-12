#!/bin/sh
# Builds Renoa.app and installs it in /Applications (or ~/Applications when
# /Applications is not writable). A build installed this way does not update
# itself; a published one does (scripts/publish-mac-app).
#
# An installed app stays the same binary until the next install, so once the
# Keychain is told to Always Allow it, it stops asking for the login password.
# A rebuild run with `cargo run` is a new binary and asks again.
set -eu
cd "$(dirname "$0")/.."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
scripts/bundle-app.sh "$work"

if [ -w /Applications ]; then
    destination=/Applications
else
    destination="$HOME/Applications"
    mkdir -p "$destination"
fi
rm -rf "$destination/Renoa.app"
ditto "$work/Renoa.app" "$destination/Renoa.app"
echo "Installed $destination/Renoa.app"
