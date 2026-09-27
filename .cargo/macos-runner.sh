#!/bin/sh
# macOS exposes the per-user temporary directory through the `/var` symbolic
# link. Renoa refuses data roots with a linked ancestor, so test and run
# processes receive the same directory by its resolved path.
TMPDIR="$(cd "${TMPDIR:-/tmp}" && pwd -P)/"
export TMPDIR
exec "$@"
