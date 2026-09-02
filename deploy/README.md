# VPS deployment

This directory contains four independent services and one transport process:

- `renoa-coordinator` carries RCP task continuity and the separate short-lived
  Host OAuth callback relay; and
- `renoa-node` executes statically bound RCP tasks through a local Host; and
- `renoa-registry` shares immutable Agent Plugin packages between Hosts; and
- `renoa-telegram` runs the Arcee personal-operator profile on Telegram; while
- `cloudflared` gives the loopback-only coordinator a public HTTPS route.

The coordinator and registry do not require each other. Both remain plaintext
and loopback-only. Cloudflare Tunnel terminates public TLS for the coordinator;
Tailscale Serve remains a private fallback and the registry's only remote
route. Neither transport is part of a Renoa protocol. Funnel is not used.

The Telegram surface is different: it makes outbound HTTPS requests to the
Telegram Bot API and opens no listener, so it does not use Tailscale Serve.

## RCP execution node

Build and install the headless Host node:

```sh
cargo build --locked --release -p renoa-node --bin renoa-node
install -m 0755 target/release/renoa-node /usr/local/bin/renoa-node
useradd --system --home-dir /var/lib/renoa-node \
  --shell /usr/sbin/nologin renoa-node
install -d -m 0700 -o renoa-node -g renoa-node /srv/renoa/node-workspaces
install -d -m 0700 -o root -g root /etc/renoa
```

Create `/etc/renoa/node.json` as root with mode `0600`. It is an exact local
Host configuration, not RCP wire data:

```json
{
  "schemaVersion": 1,
  "endpoint": "wss://renoa.live/connect",
  "model": {
    "bridge": "/opt/renoa/adapters/model-provider-node/dist/src/main.js",
    "credentialStore": "/var/lib/renoa-node/model-auth.sqlite",
    "providers": ["opencode-go"],
    "defaultProvider": "opencode-go",
    "defaultModel": "glm-5.3-flash"
  },
  "adapters": {
    "mcp": "/opt/renoa/adapters/mcp-node/dist/src/main.js",
    "mcpRegistry": "/opt/renoa/adapters/mcp-registry-node/dist/src/main.js",
    "sharedPluginRegistry": "http://<vps-magic-dns-name>:8082/"
  },
  "targets": [
    {
      "target": "workspace:example",
      "profile": "renoa.coding.alpha.v1",
      "sessionId": "<stable-session-uuid>",
      "workspace": "/srv/renoa/node-workspaces/example"
    }
  ]
}
```

Every configured adapter and model store must already exist at its absolute
path. Omit any optional adapter field that this Host does not use. The service
currently accepts the built-in Alpha and Arcee profile IDs. Each target binds
one coordinator target to one stable Host session and canonical workspace;
changing a durable binding fails closed.

On the coordinator host, create the node identity and capture its five-minute
enrollment token directly into an owner-only file:

```sh
umask 077
systemd-run --quiet --wait --pipe --collect \
  --property=DynamicUser=yes \
  --property=StateDirectory=renoa \
  --property=StateDirectoryMode=0700 \
  --property=UMask=0077 \
  /usr/local/bin/renoa-coordinator enroll-node \
  /var/lib/renoa/control.sqlite <node-uuid> > node-enrollment.json
```

Move that short-lived file to the execution Host over an authenticated private
channel, keep it mode `0600`, and exchange it once:

```sh
/usr/local/bin/renoa-node enroll \
  wss://renoa.live/connect \
  /run/renoa/node-enrollment.json \
  /etc/renoa/node-device.json
rm /run/renoa/node-enrollment.json
```

The output credential file is created as mode `0600` and is never overwritten.
The command prints only `{"status":"enrolled"}`. Install the unit after the
coordinator task has been created with the same node UUID and target:

```sh
cp deploy/renoa-node.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now renoa-node.service
journalctl -u renoa-node.service -f -o cat
```

The unit passes the config and device secret through systemd credentials, whose
runtime directory is available as `%d`. It grants writes only to the private
node state and `/srv/renoa/node-workspaces`; add another explicit
`ReadWritePaths=` entry in a drop-in before binding a workspace elsewhere.
Node/V8 needs writable executable memory, so `MemoryDenyWriteExecute` remains
off. Network loss is retried internally with bounded exponential backoff;
systemd restarts only fatal process exits.

## Arcee Telegram surface

Build and install the service binary:

```sh
cargo build --locked --release -p renoa-telegram
install -m 0755 target/release/renoa-telegram /usr/local/bin/renoa-telegram
```

Install `ripgrep` on the runtime host. Renoa uses `rg` for deterministic skill
and workspace discovery; the service fails clearly instead of silently changing
that behavior when it is unavailable. On Debian:

```sh
apt-get install ripgrep
```

Create a dedicated unprivileged account and workspace. Do not add this account
to `sudo`, `docker`, or service-management groups.

```sh
useradd --system --home-dir /var/lib/renoa-telegram \
  --shell /usr/sbin/nologin renoa-arcee
install -d -m 0700 -o renoa-arcee -g renoa-arcee /srv/renoa/arcee
install -d -m 0700 -o root -g root /etc/renoa
```

Place the BotFather token in `/etc/renoa/telegram-bot-token`, owned by root with
mode `0600`. Put the remaining explicit settings in
`/etc/renoa/telegram.env`; that file must not contain the bot token:

```text
RENOA_TELEGRAM_ALLOWED_USER_ID=123456789
RENOA_TELEGRAM_IPV4_ONLY=1
RENOA_MODEL_BRIDGE=/opt/renoa/adapters/model-provider-node/dist/src/main.js
RENOA_MODEL_AUTH_STORE=/var/lib/renoa-telegram/model-auth.sqlite
RENOA_MODEL_PROVIDER=opencode-go
RENOA_MODEL=your-model-id
RENOA_MCP_ADAPTER=/opt/renoa/adapters/mcp-client-node/dist/src/main.js
TZ=Asia/Kolkata
```

The model credential store and compiled Node adapter must already exist at
those paths. The adapter tree must be readable by `renoa-arcee`; the credential
store must be owned by and writable only to that account so OAuth refresh can
rotate safely. `TZ` selects the local clock Arcee sees on each turn and may be
changed to any valid IANA time-zone name. Optional MCP adapter and shared
registry settings use the same environment names documented in
[`renoa-telegram`](../crates/renoa-telegram/README.md).

Give the Telegram Host its own Node identity for callback-relay management.
This identity grants no task access and is not shared with `renoa-node`. Create
and consume the short-lived enrollment without printing either secret:

```sh
install -d -m 0700 -o root -g root /run/renoa
umask 077
systemd-run --quiet --wait --pipe --collect \
  --property=DynamicUser=yes \
  --property=StateDirectory=renoa \
  --property=StateDirectoryMode=0700 \
  --property=UMask=0077 \
  /usr/local/bin/renoa-coordinator enroll-node \
  /var/lib/renoa/control.sqlite <oauth-relay-node-uuid> \
  > /run/renoa/arcee-oauth-relay-enrollment.json
/usr/local/bin/renoa-node enroll \
  wss://renoa.live/connect \
  /run/renoa/arcee-oauth-relay-enrollment.json \
  /etc/renoa/arcee-oauth-relay-device
rm /run/renoa/arcee-oauth-relay-enrollment.json
```

The unit supplies that file through systemd credentials and pins the relay
origin to `https://renoa.live`. When an MCP requires OAuth, Arcee sends a
permanent Telegram message with a provider-login button; the temporary thinking
draft contains no URL. The callback lands at the public origin, while PKCE state
and tokens stay in `/var/lib/renoa-telegram`. If an MCP needs an API token or
pre-registered OAuth client, Arcee instead sends a permanent encrypted setup
link. The coordinator sees only ciphertext; the plaintext is stored by Arcee's
Host before the MCP connection is published.

Install the unit and start it:

```sh
cp deploy/renoa-telegram.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now renoa-telegram.service
journalctl -u renoa-telegram.service -f -o cat
```

The unit gives Arcee a writable private state directory and dedicated workspace,
outbound network access, and a private temporary directory. The rest of the
host filesystem is read-only, Linux capabilities are removed, and the service
account cannot use privileged service or Docker control. Node/V8 requires
writable executable memory, so `MemoryDenyWriteExecute` is deliberately absent.
This is the first operational boundary, not the final Renoa permission model.

## RCP coordinator

Build the Linux binary with the workspace's locked dependencies:

```sh
cargo build --locked --release -p renoa-control --bin renoa-coordinator
```

Install `target/release/renoa-coordinator` at
`/usr/local/bin/renoa-coordinator`, copy `renoa-coordinator.service` to
`/etc/systemd/system/`, then enable the service:

```sh
systemctl daemon-reload
systemctl enable --now renoa-coordinator.service
```

Expose the loopback listener on a private tailnet HTTPS port:

```sh
tailscale serve --bg --yes --https=8443 http://127.0.0.1:7818
```

RCP peers then connect to:

```text
wss://<vps-magic-dns-name>:8443/connect
```

If tailnet certificate issuance is temporarily unavailable, a private HTTP
Serve endpoint can prove continuity without exposing a public port:

```sh
tailscale serve --bg --yes --http=8081 http://127.0.0.1:7818
```

Peers then use `ws://<vps-magic-dns-name>:8081/connect`. Tailscale still
encrypts the network path, but browser secure-context rules may require WSS;
the HTTP endpoint is a temporary fallback, not the target deployment.

Verify both layers independently:

```sh
systemctl status renoa-coordinator.service
tailscale serve status
```

### Public RCP route

Install `cloudflared` from Cloudflare's signed package repository. Create a
remotely managed tunnel whose public hostname is `renoa.live` and whose service
is `http://127.0.0.1:7818`. Its ingress configuration must end with a catch-all
`http_status:404` rule. The coordinator remains unreachable on a public TCP
port; the tunnel connector initiates the network connection from the VPS.

Store the tunnel token—not an RCP device credential—at
`/etc/renoa/cloudflare-tunnel-token`, owned by root with mode `0600`. Install the
unit and start the connector:

```sh
cp deploy/renoa-cloudflared.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now renoa-cloudflared.service
```

The Cloudflare DNS zone needs one proxied CNAME at the apex:

```text
renoa.live -> <tunnel-id>.cfargotunnel.com
```

RCP peers then connect to `wss://renoa.live/connect`. Verify the origin and the
connector separately before enrolling a device:

```sh
systemctl status renoa-coordinator.service
systemctl status renoa-cloudflared.service
journalctl -u renoa-cloudflared.service -n 50 -o cat
```

The tunnel token authorizes only this connector. RCP devices still authenticate
independently inside the WebSocket protocol, and provider, MCP, and tool secrets
remain on their execution Host.

The service runs as a dynamic user. Systemd creates `/var/lib/renoa` with mode
`0700`, and the service umask keeps the SQLite journal owner-only. Run local
bootstrap commands inside a transient systemd sandbox so they see the same
protected state directory:

```sh
systemd-run --quiet --wait --pipe --collect \
  --property=DynamicUser=yes \
  --property=StateDirectory=renoa \
  --property=StateDirectoryMode=0700 \
  --property=UMask=0077 \
  /usr/local/bin/renoa-coordinator enroll-surface \
  /var/lib/renoa/control.sqlite <principal-uuid> <surface-name>
```

Its JSON output contains a single-use secret that expires after five minutes.
Create the first browser passkey bootstrap through the same local boundary:

```sh
systemd-run --quiet --wait --pipe --collect \
  --property=DynamicUser=yes \
  --property=StateDirectory=renoa \
  --property=StateDirectoryMode=0700 \
  --property=UMask=0077 \
  /usr/local/bin/renoa-coordinator bootstrap-passkey \
  /var/lib/renoa/control.sqlite <principal-uuid>
```

That five-minute token is entered only into the same-origin browser passkey
registration flow. The service unit pins the WebAuthn relying party to
`renoa.live` and its exact `https://renoa.live` origin.

Use the same wrapper to enroll the execution node and create its task binding:

```sh
systemd-run --quiet --wait --pipe --collect \
  --property=DynamicUser=yes \
  --property=StateDirectory=renoa \
  --property=StateDirectoryMode=0700 \
  --property=UMask=0077 \
  /usr/local/bin/renoa-coordinator enroll-node \
  /var/lib/renoa/control.sqlite <node-uuid>

systemd-run --quiet --wait --pipe --collect \
  --property=DynamicUser=yes \
  --property=StateDirectory=renoa \
  --property=StateDirectoryMode=0700 \
  --property=UMask=0077 \
  /usr/local/bin/renoa-coordinator create-task \
  /var/lib/renoa/control.sqlite \
  <task-uuid> <principal-uuid> <node-uuid> <target>
```

Enrollment output is secret. Capture it directly into an owner-only file and
exchange it immediately. These local commands do not create a remote
administration protocol.

## Current proof status

On 2026-09-01, `renoa.live` resolved through public recursive DNS and served a
valid Cloudflare-managed certificate. The remotely managed `renoa-control`
tunnel routes only that hostname to `http://127.0.0.1:7818`, followed by a 404
catch-all. The coordinator and registry still expose no public listener.

Coordinator binary
`bbb3dfe19eb4a63750f42cf03c84a7625e948aaa516bf6d4ad727dae335a58b4`
was deployed with the hardened connection limits documented in
[`rcp-json-ws-v0.md`](../docs/rcp-json-ws-v0.md). A disposable surface enrolled,
authenticated as RCP binding version 8, and completed `list_tasks` through
`wss://renoa.live/connect`. Its plaintext credential was neither printed nor
saved; the coordinator retained only the unusable digest after the client
exited.

The same public origin then carried a complete Rust Host proof. Alpha used
OpenCode Go with GLM-5.3-Flash to call `read_file` on a unique local value and
completed at task cursor 6. A separately enrolled second surface replayed the
exact first turn, submitted the next command into the same Host session, and
completed at cursor 11. The first surface reauthenticated with its own
credential and replayed that exact five-event continuation from cursor 6. All
three one-time enrollments were consumed, credentials remained process-local,
and the private disposable runner directory was removed after success.

The production node binary
`ccb0b30aef70feeb082349c191391e8a2e53b65f3c65427a43ba721034c02f06`
then replaced the disposable runner under `renoa-node.service`. It runs as its
own unprivileged account and scored `1.5 OK` under `systemd-analyze security`.
Surface A completed a real Alpha `read_file` turn at cursor 6. After a clean
service restart, independently enrolled Surface B replayed all seven existing
records, continued the same durable Host session without repeating the tool,
and completed at cursor 11. Surface A then reattached from cursor 6 and received
exactly the five-record suffix. The two disposable surface credentials and
their local cursor stores were deleted; the service's owner-protected node
credential and durable session remain deployed.

On 2026-08-12, coordinator binary
`3918d12d6ee2f40307b3a7177227e243d2add2afdec67144ee8d31cf9d8cb557`
was deployed. A trusted bootstrap created a fresh principal, Pi node, and task.
The Mac node used Pi SDK, SuperGrok, and `grok-4.5` to read and edit one confined
workspace file. The attached TypeScript surface disconnected immediately after
command admission and reconnected only after the node had durably published its
terminal event. It received a contiguous 13-event task history, 12 events by
replay, one command admission, and one completed terminal. The coordinator
remained loopback-only, and the proof used the tailnet-only port above.

## Shared Agent Plugin registry

The registry is not a remote Host or an Agent runtime. It stores only immutable
package archives and their ordered revisions. Credentials, MCP connections,
profile attachments, workspaces, and sessions remain on each Host.

Build its Linux binary from the locked workspace:

```sh
cargo build --locked --release -p renoa-registry --bin renoa-registry
```

Install `target/release/renoa-registry` at
`/usr/local/bin/renoa-registry`, copy `renoa-registry.service` to
`/etc/systemd/system/`, and enable it:

```sh
systemctl daemon-reload
systemctl enable --now renoa-registry.service
```

Expose only its loopback listener to the private tailnet. Use private HTTPS when
certificate issuance works:

```sh
tailscale serve --bg --yes --https=8444 http://127.0.0.1:7820
```

The current private HTTP fallback is:

```sh
tailscale serve --bg --yes --http=8082 http://127.0.0.1:7820
```

The registry v1 intentionally has no second application login. Tailnet
membership and ACLs are its first deployment boundary, so do not expose this
port through Funnel or a public reverse proxy. Verify service and route before
configuring a Host:

```sh
systemctl status renoa-registry.service
tailscale serve status
curl --fail --show-error http://<vps-magic-dns-name>:8082/v1/status
```

Then add the origin—not `/v1`—to every trusted Host process:

```sh
export RENOA_SHARED_PLUGIN_REGISTRY='http://<vps-magic-dns-name>:8082/'
renoa-agent plugins sync
```

The sync command's JSON reports local publications, downloads, and the durable
applied revision. The first successful response binds that Host data directory
to the registry's stable UUID. Changing the URL is safe when it routes to the
same registry state; pointing it at another registry fails closed.

On 2026-08-31, registry binary
`384752de4a643b6f6da0ae66828a45bcfb50cf816f07ebcd5ed167fd16dee9e2`
was built with Rust 1.95 on Debian Bookworm and deployed beside the existing
coordinator. The service remained IPv4-loopback-only on port `7820`; Tailscale
Serve exposed the tailnet-only HTTP fallback on port `8082`. A forced service
restart preserved the registry identity and empty revision log before its first
Host sync. The existing laptop Host then published eight immutable package
revisions; a fresh second Host pulled all eight over the tailnet and durably
advanced to revision `8`. The hardened systemd unit scored `1.3 OK` under
`systemd-analyze security` on that host.
