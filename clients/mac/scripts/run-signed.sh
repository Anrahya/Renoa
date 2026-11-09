#!/bin/sh
# Runs a build of the app signed with the "Renoa Dev" identity (any case), when this Mac
# has one. The Keychain trusts an app by its signature: an unsigned build is a
# new app each time it is built, and asks for the login password again. A
# build signed with one identity stays the same app. Test binaries run as
# they are.
set -eu
binary="$1"
shift
if [ "$(basename "$binary")" = "renoa-mac" ] \
    && security find-identity -p codesigning | grep -qi '"Renoa Dev"'; then
    codesign --force --sign "Renoa Dev" --identifier live.renoa.mac "$binary"
fi
exec "$binary" "$@"
