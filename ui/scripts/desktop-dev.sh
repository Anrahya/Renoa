#!/usr/bin/env bash
set -euo pipefail

script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repository_directory=$(cd -- "$script_directory/../.." && pwd)
configuration_directory=${XDG_CONFIG_HOME:-"${HOME:?HOME must be set}/.config"}

if ! pkg-config --exists gtk+-3.0 webkit2gtk-4.1; then
  printf '%s\n' \
    'Renoa desktop needs the Fedora Tauri development packages.' \
    'Install them with:' \
    '  sudo dnf install gtk3-devel webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel' >&2
  exit 1
fi

export RENOA_AGENT_BIN=${RENOA_AGENT_BIN:-"$repository_directory/target/debug/renoa-agent"}
export RENOA_PI_AUTH_STORE=${RENOA_PI_AUTH_STORE:-"$configuration_directory/renoa/pi-auth.sqlite"}
export RENOA_PI_BRIDGE=${RENOA_PI_BRIDGE:-"$repository_directory/nodes/pi/dist/src/model-bridge-main.js"}
export RENOA_PI_MODEL=${RENOA_PI_MODEL:-grok-4.6}
export RENOA_PI_PROVIDER=${RENOA_PI_PROVIDER:-xai}

if [[ ! -f "$RENOA_PI_AUTH_STORE" ]]; then
  printf 'Renoa credential store does not exist: %s\n' "$RENOA_PI_AUTH_STORE" >&2
  printf '%s\n' 'Authenticate first with: pnpm --dir nodes/pi auth:xai' >&2
  exit 1
fi

pnpm --dir "$repository_directory/nodes/pi" install --frozen-lockfile --ignore-scripts
pnpm --dir "$repository_directory/nodes/pi" build
pnpm --dir "$repository_directory/ui" install --frozen-lockfile
exec pnpm --dir "$repository_directory/ui" desktop:dev
