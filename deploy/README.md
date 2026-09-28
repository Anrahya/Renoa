# VPS deployment

This directory contains the supplied service units and configuration examples for
the current deployment:

- `renoa-coordinator` carries RCP task continuity and the separate short-lived
  Host OAuth callback relay; and
- `renoa-node` executes RCP tasks with the shared Host's agents; and
- `renoa-registry` shares immutable Agent Plugin packages between Hosts; and
- `renoa-host` runs the surface-independent routine scheduler; and
- `renoa-management` serves the authenticated personal Host control panel; and
- `renoa-discord` serves saved agent-channel bindings and private plugin approval; and
- `renoa-slack` and `renoa-telegram` expose the configured Host through their
  respective surfaces (`renoa-slack.service` and `renoa-slack-host.service` are
  alternative Slack deployment units); while
- `cloudflared` gives the loopback-only coordinator a public HTTPS route.

The coordinator and registry do not require each other. Both remain plaintext
and loopback-only. Cloudflare Tunnel terminates public TLS for the coordinator;
Tailscale Serve remains a private fallback and the registry's only remote
route. Neither transport is part of a Renoa protocol. Funnel is not used.

The Telegram surface is different: it makes outbound HTTPS requests to the
Telegram Bot API and opens no listener, so it does not use Tailscale Serve.

## Releases and deployment

CI builds every release; nothing deployed is built on a developer machine.
`.github/workflows/ci.yml` runs the `AGENTS.md` gates on every pull request.
Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds one
archive in a Debian 13 container (the VPS's own glibc) and publishes it as a
GitHub Release with `SHA256SUMS` and build provenance:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

`deploy/release.json` is the single definition of a release: the binaries to
build, the Node adapters to ship, and each service with the binaries it runs,
whether it runs the adapters or serves the Control Room, and its companion
units. `deploy/package-release` builds the archive from it; the archive's own
`release.json` adds the tag, commit, build time and each binary's SHA-256.

On the VPS, `renoa-deploy` installs a release:

```sh
renoa-deploy install v0.2.0
renoa-deploy status
```

It downloads the archive, checks it against `SHA256SUMS` and every binary
against the manifest, then unpacks it into `/opt/renoa/releases/<tag>/`. It
refuses a release that lacks a binary of an installed service. Only services
whose unit is installed on this host are updated, and only those whose
binaries, unit files, adapters or Control Room changed are restarted, in the
order `release.json` lists them. Restarting `renoa-host` or `renoa-node` first
waits for running agent turns to finish (`--drain-timeout`, 600 s by default).
Before switching, it snapshots the Host, coordinator, node and Discord SQLite
databases with SQLite's backup API, and copies `config/`, into the release
being replaced. It then carries that release's own hashed Control Room assets
forward, so a browser holding the previous page still loads, installs changed
unit files, and switches.

`/opt/renoa/current` points at the running release and `/opt/renoa/previous`
at the one backup: the release it replaced, with its snapshot. The binaries in
`/usr/local/bin`, `/opt/renoa/adapters` and `/opt/renoa/control-room` are
symlinks through `current`, so a switch is one rename. The first install adopts
a hand-installed layout as `releases/legacy` without changing what runs.

After the switch it checks that every service that was running stays active
through a ten-second settle window, that management answers an
unauthenticated request with 401 and a foreign origin with 403, and that the
Host inspects. Then it deletes every other release and the old
`/opt/renoa/previous-release`, writes `/opt/renoa/current-release.json`, and
installs its own new copy at `/usr/local/sbin/renoa-deploy`. A failed check
leaves the new release running, keeps every release and backup, and prints the
failures. There is no automatic rollback: recovery is an explicit owner action,
pointing `current` back at `previous` and restoring its snapshot. Live
conversation history, credentials and current Host databases are application
data, not release backups.

The first deploy with this tool runs it from the archive, since
`/usr/local/sbin/renoa-deploy` does not exist yet:

```sh
curl -fsSLO https://github.com/Anrahya/Renoa/releases/download/v0.2.0/renoa-v0.2.0-linux-x86_64.tar.gz
tar xzf renoa-v0.2.0-linux-x86_64.tar.gz renoa-v0.2.0/renoa-deploy
./renoa-v0.2.0/renoa-deploy install v0.2.0
```

Installing a service for the first time, enrolling it, and writing its
configuration stay manual steps; the sections below describe them.

## Optional MCP Code Mode worker

Code Mode is enabled Host-wide by its worker configuration. It uses the Node.js
MCP adapter for remote calls and one
exact Monty subprocess binary; it does not download a runtime during a turn.
This release uses `monty-pool`/`monty-types` `1.0.0-beta.2` and the Monty
upstream [commit `64662cc`](https://github.com/pydantic/monty/commit/64662cc567c5c0e515121cc1ba42cb115a19ffd6)
(MIT license). The official Linux x86-64 `manylinux_2_28` worker comes from
`pydantic-monty-runtime==1.0.0b2`. Its wheel SHA-256 is
`f2b31e47835f13b0d2735c4f30470eb796c6c42fe381b6ece19cac148c784c5b`; the extracted
`monty` executable SHA-256 is
`f596526655da1026bfbd928e4fa26bdbbe461e3130a351cea77208acd2bae140`.
The Host verifies the executable hash before opening mutable Host state and
refuses a different worker. `rust-toolchain.toml` pins the build to Rust 1.98.1;
rustup installs it on the first `cargo` command in the checkout.

Stage and verify the worker before setting a Host configuration field:

```sh
monty_stage=$(mktemp -d)
python3 -m pip download --pre --no-deps --only-binary=:all: \
  --dest "$monty_stage" 'pydantic-monty-runtime==1.0.0b2'
monty_wheel="$monty_stage/pydantic_monty_runtime-1.0.0b2-py3-none-manylinux_2_28_x86_64.whl"
sha256sum "$monty_wheel"
unzip -j "$monty_wheel" \
  'pydantic_monty_runtime-1.0.0b2.data/scripts/monty' -d "$monty_stage"
sha256sum "$monty_stage/monty"
sudo install -D -o root -g root -m 0755 "$monty_stage/monty" \
  /opt/renoa/libexec/monty/f596526655da1026bfbd928e4fa26bdbbe461e3130a351cea77208acd2bae140/monty
```

Compare both printed hashes with the values above before installing. Use an
app-owned immutable path; do not replace that executable in place while work
is active. Set `code_mode_worker` to the installed absolute path in a Host,
Slack, or Discord JSON configuration; use `adapters.codeModeWorker` in the
RCP node JSON, or `RENOA_CODE_MODE_WORKER` for ACP, Telegram, and the local CLI.
Configuring the worker enables Code Mode for every agent and hides direct
`tool_execute` behind its durable invocation boundary. An MCP adapter is required
for remote MCP calls; Host plugin calls work without it. Without a worker, agents
use direct `tool_execute`.
Drain active work before upgrading the worker, and retain only the immediately
previous release's consolidated backup as described above.

## Discord and owner onboarding

Use one canonical Host home, defaulting to `~/.renoa`; the supplied owner-panel
and Discord examples use `/home/renoa/.renoa` with the `renoa-arcee` service account.
Point every worker and the panel at that same home and enabled provider set.
The owner panel's `models` configuration names its existing credential store.
Keep browser identity storage through a Host reset so login survives.
The supplied
coordinator unit uses `state/coordinator.sqlite3` in this same home; an existing
installation must move its identity database with SQLite backup before switching
that unit. The Host unit reads `config/host.json` and the relay credential file.

The Discord worker runs no agents. Each channel's conversation is an RCP task,
executed by the `renoa-node` that advertises the channel's agent (see
[RCP execution node](#rcp-execution-node)); install that node first. Build the
worker and install its service and path units:

```sh
cargo build --locked --release -p renoa-discord
cp deploy/renoa-discord.service deploy/renoa-discord.path /etc/systemd/system/
systemctl daemon-reload
```

Enroll the worker as a surface of the Host's owning principal, the same
principal that owns the node, and exchange the token once:

```sh
umask 077
sudo -u renoa-arcee /usr/local/bin/renoa-coordinator enroll-surface \
  /home/renoa/.renoa <owner-principal-uuid> discord > /run/renoa/discord-enrollment.json
sudo -u renoa-arcee /usr/local/bin/renoa-node enroll \
  ws://127.0.0.1:7818/connect \
  /run/renoa/discord-enrollment.json \
  /home/renoa/.renoa/credentials/discord-rcp-device.json
rm /run/renoa/discord-enrollment.json
```

Save `renoa-discord.config.example.json` as `config/discord.json` (owner-only).
It names the home, the loopback coordinator endpoint, and that credential file;
it holds no Discord identity and no model or adapter settings. Enable the path
unit, not the service: it starts `renoa-discord.service` once the owner connects
a bot.

```sh
systemctl enable --now renoa-discord.path
```

Create the agent through Agents → Create agent in the Control Room. No native
machine grants are selected by default. Every agent gets plugin management and
discovery. Its Configure page shows the saved instructions and model selection.

Create a bot in the Discord Developer Portal, turn on Message Content Intent on its
Bot page, and copy its token. On the agent's Configure page, paste the token and
check it. Invite the bot with the offered Administrator link, check again, choose
the server and connect. Renoa requires Administrator in that server. Management
commits the token, server, application owner and this default agent once, to the
owner-only `credentials/discord.json`; the browser never stores the token. That
file's appearance starts the worker. Then choose channels for each agent.

A saved binding proves channel validation and durable routing, not present bot
connectivity or send permission. New messages in that channel route to the
selected agent without a mention; reassignment starts a new task and preserves
earlier admitted work. The default agent answers direct messages and mentions
elsewhere. A message whose agent's node is offline is answered as not sent and
must be resent; it is never held. Replies to commands submitted to the same task
from another surface are posted to the channel with their origin.

While a command runs, the channel shows the bot typing. Once the agent calls a
tool, one progress message answers the command and is edited in place, listing
each tool call and the intermediate messages that led to it. The answer itself
arrives as the reply, and the progress message is then deleted; so is the
message of a command idle for 20 minutes. Progress is transient: only the posted
message's identity is stored, a command that calls no tool only shows typing,
and after a restart progress resumes from the next task record in the same
message, or the message is deleted if its command finished meanwhile.

OAuth and credential links go to the application owner's DM. The executing node
sends them directly with the Host's Discord connection, never through the RCP
task journal, because a credential-setup link carries the key that keeps the
relayed credential unreadable to the coordinator. Enable DMs before requesting
plugin authorization. Unknown delivery is not automatically repeated; check the
DM before restarting setup.

The connection cannot be changed from the Control Room. To connect a different bot,
server or default agent, stop `renoa-discord.path` and `renoa-discord.service`,
then remove `credentials/discord.json` and `state/surfaces/discord`; the latter
pins the previous identity and holds its bindings and task cursors.

Discord store schema 5 records posted progress messages, so a restart still
deletes them; upgrading from schema 4 adds the empty table.

Discord store schema 4 moved conversations into RCP tasks. Upgrading keeps the
identity, channel bindings, gateway cursor, and message deduplication, and drops
in-process sessions, queued turns, replies, and setup-delivery records.

An installation configured before Control Room onboarding must drop
`discord_config` from `config/management.json`, replace `config/discord.json` with
the runtime-only example, delete `credentials/discord-bot-token`, and connect again.
If the new connection names a different server, application owner or default agent,
the worker refuses the stored identity; remove `state/surfaces/discord` as above.

## Personal Host observation

To inspect the existing personal Host without starting models or acquiring agent
execution ownership, run the current binary as the Host's OS user:

```sh
sudo -u renoa-arcee /usr/local/bin/renoa-host inspect /home/renoa/.renoa
```

This command emits metadata JSON only; it does not serve a public endpoint or
change deployment configuration. Use the actual shared Host data root, not the
separate RCP node's root. The command fails for absent/incompatible catalogs and
reports individual unreadable sessions without inventing idle states.

## Personal control panel

`renoa-management` is a separate management API and static panel service. It opens the
existing personal Host and asks the loopback identity service to validate browser
sessions. It never executes agent turns or reads the coordinator's private database.
With `models` configured it queries the provider catalog for owner creation.
The initial panel observes agents, sessions, schedules and shared inventory. The
browser exposes owner pause/resume of existing automations. Owners can
create agents with explicit instructions, models and native grants,
connect the Discord bot, then save Discord channel bindings. Full definition and automation editors remain
subsequent work.

Build the coordinator, management adapter and production assets:

```sh
cargo build --locked --release -p renoa-control --bin renoa-coordinator -p renoa-management
npm --prefix surfaces/control-room run build
```

Install the management binary at `/usr/local/bin/renoa-management` and copy only
`surfaces/control-room/dist/client/` into `/opt/renoa/control-room/`. This is a
dedicated public asset directory, never the Host data root or source checkout.
Retain hashed assets for the current and immediately previous release so an
already-open browser can finish loading its version. Remove assets from older
releases; older browser pages need a reload. The `?preview` example-data route is development-only;
`?tasks` retains the earlier RCP task surface.

Use `renoa-management.config.example.json` for `/home/renoa/.renoa/config/management.json` with
root ownership and mode `0600`. Replace both placeholder UUIDs: `host_id` is the
existing Host identity from `renoa-host inspect`; `owner_principal_id` is the one
human principal enrolled for this Host's panel. Never infer ownership from the
first legacy RCP task. Keep that principal and configuration in the Host's backup.
Set `public_origin` to the exact external HTTPS origin, such as
`https://renoa.live`. Every management write requires this Origin header and an
authenticated owner cookie; the server does not trust forwarded headers to select
the origin. The only development exception is HTTP `localhost`.

This release requires Host schema 33. A schema 28–32 catalog upgrades in place
when the Host opens it, dropping the retired GitHub review tables. An earlier
data root cuts agent-owned storage over to the canonical agent definition. That
cutover is not a migration: it discards the previous agent rows, routines and
sessions, and it runs only through the explicit reset described in
[`docs/renoa-host-v0.md`](../docs/renoa-host-v0.md). Starting a normal Host
process against an earlier data root fails closed with the reset command in the
error, so stop all readers/writers of the shared Host catalog, including
management, Slack and Telegram workers, then run:

```sh
renoa-host /home/renoa/.renoa/config/host.json reset /opt/renoa/previous-release-staging/host
```

The reset copies the whole data root to that directory first (refusing a
non-empty backup, or one inside the data root), then applies the cutover. Host
identity, MCP integrations, connections, catalogs, authorizations and
credentials, installed plugins, immutable skill revisions and sources, the
shared registry, and every workspace file are preserved; agent-owned rows, the
session directories, and the agent document roots are not.
Provision the configured agent again after the reset:

```sh
renoa-host /home/renoa/.renoa/config/host.json provision /etc/renoa/bootstrap-agent.json
```

Observation and owner-control modules deliberately do not migrate or reset
storage themselves. The node daemon runs in the shared Host and keeps only its
derived ledger at `state/node.sqlite3`; its device credential and the
coordinator's task bindings live outside the reset roots and
survive. Each surface store is its own step, as listed in
`docs/renoa-host-v0.md`. The previous binaries and matching database snapshot
are the single backup at `/opt/renoa/previous` described above. Browser login
storage is separate and does not need to be reset for this Host cutover.

Back up the coordinator SQLite database with SQLite's backup API before installing
the new coordinator binary. It upgrades the identity database to schema 12 and
keeps passkeys and hashed remembered sessions there. Schema 12 records each
node's owning principal: the upgrade adopts the owner of a node whose existing
tasks all belong to one principal, and a node shared by several principals keeps
serving its tasks without an owner. A pre-upgrade binary cannot open schema 12;
any owner-requested recovery requires the matching identity
snapshot and binary from that same backup. Install `renoa-management.service`, then reload systemd, restart the
coordinator and enable the management service. The example runs as the existing
Host OS user, `renoa-arcee`; use the actual Host owner on another machine.

Preserve all existing tunnel rules and add these `renoa.live` path routes before
the coordinator catch-all:

| Path regex | Loopback destination |
| --- | --- |
| `^/v1/host(/.*)?$` | `http://127.0.0.1:7819` |
| `^/$` | `http://127.0.0.1:7819` |
| `^/index\\.html$` | `http://127.0.0.1:7819` |
| `^/assets/.*$` | `http://127.0.0.1:7819` |

Keep `/v1/identity/*`, `/connect`, credential intake and OAuth callbacks routed
to the coordinator. Both listeners
remain plaintext and loopback-only behind the same HTTPS origin. Cloudflare's
[tunnel configuration API](https://developers.cloudflare.com/api/resources/zero_trust/subresources/tunnels/subresources/cloudflared/subresources/configurations/methods/update/)
replaces the full configuration, so read and preserve the current rules first.

The panel defaults to direct browser pairing. Use `pair-browser` in place of
`bootstrap-passkey` in the systemd wrapper below, with the configured owner
principal. Capture its 30-minute output in an owner-only file; enter the code at
`https://renoa.live` → “Pair this browser.” For a phone or another browser with a
passkey provider, use `bootstrap-passkey` and “Set up a passkey on this device.”
The two codes authorize different operations and cannot substitute for each other.
Do not put bootstrap tokens in URLs, Git, logs or chat. Each browser signs in once;
the `__Host-renoa_session` cookie is Secure, HTTP-only, same-site and remembered
for 180 days, renewed after half that lifetime during use. The server stores only
its hash. Ordinary restarts and source-IP changes do not invalidate it. A revoked,
expired or deleted cookie requires another pairing code or a passkey sign-in. The existing Slack/Telegram
credentials are independent and require no new enrollment for this panel.

To revoke all remembered browser sessions and existing pairing codes for the owner
through trusted local administration, run `renoa-coordinator revoke-browser-logins <renoa-home>
<principal-uuid>` as the identity store's OS owner using the same systemd wrapper
as enrollment. Revocation includes unused codes and commits atomically with
session removal; fresh codes can be issued afterward. Other owners are unaffected.
The panel's sign-out revokes only its current browser. Neither
operation revokes native devices, an already-open RCP WebSocket, or an already
issued transport ticket; unused tickets expire after 60 seconds.

Verify the public shell and login prompt, anonymous inventory rejection, initial
enrollment, refresh/restart without another ceremony, logout rejection, and the
real Host UUID/counts. Network/storage failures must show a stale view and retry,
not become a successful empty inventory or an invalid-password screen. Replacing
the machine requires restoring both Host state/credentials and identity state,
and retaining the passkey origin; a DNS name alone does not preserve the system.

## RCP execution node

The node executes RCP tasks with the shared Host's agents. It runs as the Host
account against `/home/renoa/.renoa`, the same installation root as
`renoa-host`, `renoa-management`, and the Discord surface, so an agent keeps its
plugins, credentials, and workspace whichever surface reaches it.

Build and install the binary:

```sh
cargo build --locked --release -p renoa-node --bin renoa-node
install -m 0755 target/release/renoa-node /usr/local/bin/renoa-node
```

Create `/home/renoa/.renoa/config/node.json` owned by `renoa-arcee` with mode
`0600`. It is local Host configuration, not RCP wire data. Its model and adapter
settings must match the Host's other services:

```json
{
  "schemaVersion": 4,
  "endpoint": "wss://renoa.live/connect",
  "model": {
    "bridge": "/opt/renoa/adapters/model-provider-node/dist/src/main.js",
    "credentialStore": "/home/renoa/.renoa/credentials/models.sqlite3",
    "providers": ["opencode-go"],
    "defaultProvider": "opencode-go",
    "defaultModel": "<model-id>"
  },
  "adapters": {
    "mcp": "/opt/renoa/adapters/mcp-client-node/dist/src/main.js",
    "mcpRegistry": "/opt/renoa/adapters/mcp-registry-node/dist/src/main.js",
    "sharedPluginRegistry": "http://127.0.0.1:7820/",
    "oauthRelay": {
      "origin": "https://renoa.live",
      "credentials": "/home/renoa/.renoa/credentials/oauth-relay-device"
    }
  }
}
```

Every configured adapter and credential file must already exist at its
absolute path. Omit any optional adapter field that this Host does not use.

There is no target list. The node advertises every agent in its Host as the
target `agent:<agent-uuid>` and polls the Host every five seconds, so an agent
created in the Control Room becomes available without a restart. Each task
runs in its agent's own Host workspace and receives its own Host session the
first time it executes; the node ledger (`state/node.sqlite3`) records that
session for the task's later commands. A recorded task whose target no longer
names an agent workspace refuses startup. A task whose agent was removed fails
its next command instead of stopping the node.

Schema 4 removed `targets`; schema 3 had removed each target's `sessionId`. A
document of an earlier schema is refused and names the field to remove.

On the coordinator host, create the node identity for its owning principal, the
only principal that may open new tasks on the node, and exchange the
five-minute enrollment token once:

```sh
umask 077
sudo -u renoa-arcee /usr/local/bin/renoa-coordinator enroll-node \
  /home/renoa/.renoa <node-uuid> <owner-principal-uuid> > /run/renoa/node-enrollment.json
sudo -u renoa-arcee /usr/local/bin/renoa-node enroll \
  wss://renoa.live/connect \
  /run/renoa/node-enrollment.json \
  /home/renoa/.renoa/credentials/node-device.json
rm /run/renoa/node-enrollment.json
```

The output credential file is created as mode `0600` and is never overwritten.
The command prints only `{"status":"enrolled"}`. Then install the unit:

```sh
cp deploy/renoa-node.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now renoa-node.service
journalctl -u renoa-node.service -f -o cat
```

The unit passes the config and device secret through systemd credentials, whose
runtime directory is available as `%d`, and grants writes only to the Host
root. Node/V8 needs writable executable memory, so `MemoryDenyWriteExecute`
remains off. Network loss is retried internally with bounded exponential
backoff; systemd restarts only fatal process exits.

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
RENOA_TELEGRAM_AGENT_ID=<provisioned-agent-uuid>
RENOA_TELEGRAM_IPV4_ONLY=1
RENOA_MODEL_BRIDGE=/opt/renoa/adapters/model-provider-node/dist/src/main.js
RENOA_MODEL_AUTH_STORE=/var/lib/renoa-telegram/model-auth.sqlite
RENOA_MODEL_PROVIDER=opencode-go
RENOA_MODEL=your-model-id
RENOA_MODEL_REASONING=high
RENOA_MCP_ADAPTER=/opt/renoa/adapters/mcp-client-node/dist/src/main.js
TZ=Asia/Kolkata
```

The model credential store and compiled Node adapter must already exist at
those paths. The adapter tree must be readable by `renoa-arcee`; the credential
store must be owned by and writable only to that account so OAuth refresh can
rotate safely. `RENOA_TELEGRAM_AGENT_ID` is required and names the provisioned
agent Arcee executes: the `id` that the shared Host's provision command
printed. Without it the unit restart-loops at startup. The Telegram surface
store binds that agent and refuses an older surface schema by name, so after a
Host cutover delete `/var/lib/renoa-telegram/surfaces/telegram/` (the model
credential store outside it is not derived and must survive) and pair the bot
again. `TZ` selects the local clock Arcee sees on each turn and may be
changed to any valid IANA time-zone name. Optional MCP adapter and shared
registry settings use the same environment names documented in
[`renoa-telegram`](../crates/renoa-telegram/README.md).

Give the Telegram Host its own Node identity for callback-relay management.
This identity grants no task access and is not shared with `renoa-node`. Create
and consume the short-lived enrollment without printing either secret:

```sh
install -d -m 0700 -o root -g root /run/renoa
umask 077
sudo -u renoa-arcee /usr/local/bin/renoa-coordinator enroll-node \
  /home/renoa/.renoa <oauth-relay-node-uuid> <owner-principal-uuid> \
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

The coordinator runs as the Host OS account and uses the same private Renoa home.
Create that home with mode `0700`; the service umask keeps SQLite journals
owner-only. Run local bootstrap commands as that account:

```sh
sudo -u renoa-arcee /usr/local/bin/renoa-coordinator enroll-surface \
  /home/renoa/.renoa <principal-uuid> <surface-name>
```

Its JSON output contains a single-use secret that expires after five minutes.
Create the first browser passkey bootstrap through the same local boundary:

```sh
sudo -u renoa-arcee /usr/local/bin/renoa-coordinator bootstrap-passkey \
  /home/renoa/.renoa <principal-uuid>
```

The browser bootstrap has a 30-minute window for first-time setup and is consumed
when a registration ceremony starts. That token is entered only into the same-origin browser passkey
registration flow. The service unit pins the WebAuthn relying party to
`renoa.live` and its exact `https://renoa.live` origin.

For direct browser pairing, run the same command with `pair-browser` instead of
`bootstrap-passkey`. Pairing requires existing, migrated identity storage. A code
admits one browser; a retry from that same page can recover a lost response without
creating another login. Keep the page open when retrying. Revoked/logged-out sessions
cannot be restored by replaying their pairing code.

Use the same OS account to enroll the execution node and create its task binding:

```sh
sudo -u renoa-arcee /usr/local/bin/renoa-coordinator enroll-node \
  /home/renoa/.renoa <node-uuid> <owner-principal-uuid>

sudo -u renoa-arcee /usr/local/bin/renoa-coordinator create-task \
  /home/renoa/.renoa \
  <task-uuid> <principal-uuid> <node-uuid> <target>
```

Enrollment output is secret. Capture it directly into an owner-only file and
exchange it immediately. These local commands do not create a remote
administration protocol.

## Current proof status

The dated paragraphs below are deployment receipts, not declarations of the wire
version compiled by the current checkout. Current code requires RCP JSON/WebSocket
binding version 10; the recorded version-8 proof establishes only the deployment
state observed on 2026-09-01.

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

## Shared Host schema readers

Build all readers of the shared Host schema together:

```sh
cargo build --release -p renoa-local -p renoa-slack -p renoa-telegram --bin renoa-host --bin renoa-slack --bin renoa-telegram
pnpm --dir adapters/model-provider-node build
```

Stop the Host, Slack and Telegram services and back up the consistent Host data
root before the new Host brings it to schema 33. Install the new binaries
atomically and replace the model adapter's built `dist` files. Do not resume an
older reader against the upgraded database. Keep the matching database snapshot and binaries
inside the single previous-release backup. Any owner-requested recovery must
use that matching set; do not restore binaries automatically or retain older sets.

## Shared Agent Plugin registry

The registry is not a remote Host or an Agent runtime. It stores only immutable
package archives and their ordered revisions. Credentials, MCP connections and
their agent bindings, workspaces, and sessions remain on each Host.

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

Then add the origin—not `/v1`—to every trusted Host process. `renoa-agent
plugins sync` assembles the full ACP launch configuration from the environment,
so it needs the Host data root, the provider settings, and the provisioned
agent id alongside the registry origin:

```sh
export RENOA_HOME=/var/lib/renoa-telegram
export RENOA_MODEL_BRIDGE=/opt/renoa/adapters/model-provider-node/dist/src/main.js
export RENOA_MODEL_AUTH_STORE=/var/lib/renoa-telegram/model-auth.sqlite
export RENOA_MODEL_PROVIDER=opencode-go
export RENOA_MODEL=your-model-id
export RENOA_AGENT_ID=<provisioned-agent-uuid>
export RENOA_SHARED_PLUGIN_REGISTRY='http://<vps-magic-dns-name>:8082/'
renoa-agent plugins sync
```

`RENOA_AGENT_ID` is the same provisioned agent id the Telegram service runs.
Run the command as the account that owns the Host data directory; it fails
closed when any required setting above is absent.

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
