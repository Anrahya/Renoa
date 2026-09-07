# Renoa Slack surface

A personal Slack operator using Socket Mode and the existing Arcee profile.
Slack owns presentation and event delivery. The Host owns the durable Agent and
assembles sessions; the kernel owns each turn, its tools, and recovery. No Slack
policy or wire types are added to the kernel or RCP.

## Conversation behavior

This slice accepts text messages. Only the configured workspace operator can submit work. Bot messages, edits,
other users, and unbound channel discussions are ignored. Send Arcee a DM for a
continuing conversation. In a channel where Arcee has been invited, mention her
to start a thread, then continue in that thread without another mention. Each
channel thread has an isolated session; replies threaded inside a DM retain the
DM session. Conversations initially belong to the configured Arcee Agent and
can select a persisted specialist. Each Host specialist also gets its own private
channel, with the operator invited automatically. Plain messages in that channel
need no mention or `!agent` command. All messages there, including Slack thread
replies, share its continuing conversation; `!new` starts a fresh one.

Commands are ordinary messages (not Slack slash commands):

- `!agent`: list the first 20 specialists, IDs, and channel setup statuses.
- `!agent <id>`: optional manual routing in a DM or ordinary channel thread.
- `!agent arcee`: return to Arcee in a DM or ordinary channel thread.
  Dedicated specialist channels retain their assigned agent.
- `!new`: start a fresh session for the currently selected agent.
- `!status`: inspect session, model, and context usage.
- `!model [id]`: list models or select one.
- `!reasoning [level]`: list levels or change reasoning.
- `!compact`: explicitly compact the current session.
- `!cancel`: stop the earliest outstanding model turn in this conversation.
- `!help`: show the controls.

Arcee uses the existing personal-operator recipe and full access through its
configured tools and workspace. One worker serializes requests across this
surface and holds at most one live session handle. Arcee's `bot_manage` tool
creates persistent specialists with their own instructions, selected tools,
and existing Host connections. A specialist uses a working directory at
`<Host data>/bot-workspaces/<agent-id>`. The recipe is immutable in this slice;
schedules and cross-machine execution migration
remain future work. It uses normal Slack messaging and does not require Slack
AI or paid workflow features.

Each new prompt carries a concise adapter-owned context block identifying Slack
and describing automatic specialist-channel provisioning, the distinction from
Slack MCP, and current limitations. Schema 4 snapshots that block in the same
admission transaction as the user's request. Execution and cancellation consume
the stored content; restarted work never picks up a different prompt template.
Legacy requests retain their original content. This block is appended to the
user turn, so later turns preserve earlier prompt prefixes. Shared user memory
is not authoritative for the currently active interface.

## Private specialist channels

A supervised Slack provisioning task discovers Host specialists at startup,
after Slack turns, and once per minute. This is a surface projection of the Host
inventory, including bots created through another surface. It never makes Slack
channel IDs part of the Host recipe. Ready channels use short job names, such as
`x-desk`, `news`, or `research`. Collisions get small suffixes such as `news-2`.
Ask Arcee to rename a specialist through `bot_manage`; the Host display name
changes and Slack renames the existing channel, preserving its history and routing.
Manual Slack renames remain until the Host display name changes again.

Schema 7 stores each desired label before renaming. Creation still uses an
internal name containing the Agent ID until routing and invitation are ready;
that stable identity allows recovery of ambiguous creation. Labels are then
applied by channel ID. A lost rename response is reconciled by reading that
same channel before retrying; a definitive name collision reserves another
short label. The internal creation name is not the final visible name.

Slack schema 3 retains setup intent, channel ID, state, and bounded API errors.
Creation intent commits before calling Slack. An interrupted or ambiguous create
is recovered by paginated lookup matching the stable name, bot creator, and
private/nonarchived state; it never issues another create while the result is
unknown. If no match is visible, setup remains unresolved for operator inspection,
including a crash before dispatch. Inspect Slack before repairing such an intent;
blindly clearing it could create a duplicate. Routing commits before inviting
the operator. A lost invitation response is retried, accepting `already_in_channel`.
Rate limits honor Slack's delay. Missing scopes remain visible through `!agent`
and `inspect`; ordinary conversations continue running.

The installed app needs `groups:write` to create private channels and invite the
operator, and `groups:read` to reconcile creation after interrupted delivery.
For an existing app, add these scopes using the updated manifest or OAuth &
Permissions page, then reinstall the app in the workspace. Updating a local
manifest alone does not grant scopes to the installed token.

## Setup

1. Create an app at <https://api.slack.com/apps> using
   `deploy/slack-app-manifest.json`. Install it into your workspace.
2. Copy its bot token (`xoxb-`). Under Basic Information, create an app-level
   token (`xapp-`) with `connections:write`. Store each in a separate regular
   file with mode `0600`. Keep token values out of config JSON and Git.
3. Copy your Slack Member ID from your profile. Copy
   `deploy/renoa-slack.config.example.json` to a private launch config, replace
   every placeholder and path, and generate one stable Agent UUID with
   `uuidgen`. Reuse that UUID after restarts.
4. Point the model bridge/auth store at your existing Renoa installation.
   Arcee's current recipe requires `opencode-go` at startup. The current Arcee recipe filters its catalog to that provider. Optional settings are `reasoning`,
   `mcp_adapter`, `mcp_registry_adapter`, `shared_plugin_registry`, and
   `oauth_relay: {"origin": "https://renoa.live", "device_credential_file": "/absolute/path"}`.
   If this Slack app also authorizes the Slack MCP connection, its OAuth &
   Permissions → Redirect URLs must include the relay callback. The manifest
   includes `https://renoa.live/v1/oauth/callback`; replace it when using a
   different relay origin. For an existing app, add the full callback URL and
   select Save URLs. Updating this file alone does not update the live app.
5. Build and launch:

   ```sh
   cargo build --release -p renoa-slack
   target/release/renoa-slack run /absolute/path/to/slack.json
   ```

For a user service, install the binary in `~/.local/bin`, save the config as
`~/.config/renoa/slack.json`, and install `deploy/renoa-slack.service` under
`~/.config/systemd/user/`. Enable with `systemctl --user enable --now renoa-slack`.
A VPS user service needs lingering enabled to remain alive after logout.

### Sharing the existing VPS Host

Use `deploy/renoa-slack-host.service` as the system service
`renoa-slack.service` when joining the existing Telegram deployment. It runs as
the same `renoa-arcee` OS account and uses the existing Host data directory,
historically named `/var/lib/renoa-telegram`. The directory name does not assign
ownership to Telegram. Keep all processes opening this Host on compatible
builds; back up the Host and upgrade its surfaces together when its catalog
schema changes.

Save the launch JSON as a root-owned `0600` file at `/etc/renoa/slack.json` with:

- `data_directory`: `/var/lib/renoa-telegram`
- `workspace`: `/srv/renoa/arcee`
- model/MCP adapter and auth-store paths from the existing VPS installation
- `shared_plugin_registry`: the existing registry origin, if configured
- `bot_token_file`: `/run/credentials/renoa-slack.service/slack-bot-token`
- `app_token_file`: `/run/credentials/renoa-slack.service/slack-app-token`
- `oauth_relay.origin`: the existing relay origin
- `oauth_relay.device_credential_file`:
  `/run/credentials/renoa-slack.service/oauth-relay-device`

Store the two Slack tokens in root-owned `0600` files at
`/etc/renoa/slack-bot-token` and `/etc/renoa/slack-app-token`. The service loads
private copies of the tokens, launch JSON, and existing
`/etc/renoa/arcee-oauth-relay-device` credential, so the service account does not
need access to the private `/etc/renoa` directory.
The Agent UUID and allowed Slack Member ID are stable launch settings.

Both surfaces now use the same Arcee profile, enabled MCP connections, private
OAuth store, installed plugins, and profile documents. New profile attachments
are visible without a surface restart. The Host's extension inventory also
lets other profiles enable an existing connection or reuse an installed
package's skills by digest; neither operation repeats OAuth.

Stop an existing Slack daemon before starting this one: two independent Socket
Mode consumers must not split events between separate Hosts. Existing local
Slack sessions remain in their original Host; changing a launch path does not
migrate their bindings or history. Cross-Host session transfer is not part of
this deployment.

The configured data root is shared with the Host catalog. Slack also binds its
own database to the exact Host, Agent, workspace, bot, and allowed user. Changing
those bindings accidentally fails startup rather than reusing conversations
under a different identity. Event installation authorizations must match the
authenticated bot before admission; a socket token from a different app cannot
submit work through this bot. This slice supports one personal workspace
installation, not enterprise or multiple-installation routing.

## Admission, execution, and delivery

`slack.sqlite3` uses WAL, full synchronous commits, and an exclusive daemon lease.
A Socket Mode envelope is acknowledged only after an accepted request and its
session binding commit. Transport envelope IDs are not operation IDs: event IDs
and channel/message timestamps deduplicate retries and overlapping Slack event
subscriptions. Conflicting content under an existing identity is rejected.
Ignored unbound messages also retain their disposition, so a later thread
binding cannot turn a retry into a new request. Cancellation intent commits before signalling an active worker. `!new` rotates
only the binding used by later admissions. `!agent <id>` also starts a fresh
session, persisting its target Agent in the same admission transaction. Earlier
queued work retains its original target. Unknown IDs leave the current binding
unchanged. Slack schema 2 adds per-session Agent bindings; migrated schema 1
sessions continue to resolve to the configured operator. Use a separate channel
thread to retain another independently addressable conversation.

A restart requeues interrupted execution with its original session/request IDs
and original observation time. The real kernel replays completed outcomes and
reconciles unfinished operations. Infrastructure failure leaves the request
pending for restart; it is not converted to a false terminal outcome.

The initial progress message is receipted separately. Updates to its known Slack
timestamp are repeatable. Changed previews refresh at most every 1.25 seconds,
retain the first 3,500 characters, and keep text visible above tool/retry status.
Identical previews do not issue another update. Progress is joined before final
delivery. These are periodic message edits, not Slack's thread-only native text
streams. Private reasoning and raw provider
payloads are not published. Extension credential setup and provider authorization
each produce a separate action message in a DM; progress and final answers never
overwrite those messages. Schema 5 persists a request/tool-call/stage identity,
content fingerprint, delivery status, and Slack receipt before acknowledging
successful delivery. Secret-bearing URLs remain in Host recovery state and are
not stored in the Slack database. Replayed tool updates reuse the receipt. A
rate-limited post can retry; an ambiguous post is retained as unknown and is not
blindly repeated. Delivery failures cancel the waiting operation and surface a
recovery instruction. `inspect` includes these action delivery records.
In channels, setup stops with an instruction to continue privately; setup URLs
are never published into channels.

Final output commits before delivery and is split into Unicode-safe chunks.
Each new post is marked in flight before calling Slack. Rate limits honor
`Retry-After`. Model-provider retries update the working message with the wait
and next attempt; `!cancel` can interrupt the wait. Exhausted model rate limits
explain that automatic retries stopped. Completed tools are not replayed.
If a new post's outcome is ambiguous, it becomes `unknown` and
is not automatically repeated: Slack may already have accepted it. Later chunks
of that result remain blocked until the missing prefix is resolved manually. Updates to
a known message can safely retry. This is not an exactly-once delivery claim.

Inspect retained results and explicit delivery failures while the daemon runs:

```sh
renoa-slack inspect /absolute/path/to/slack.json
```

This returns the latest 20 requests plus up to 100 unknown, failed, or blocked chunks. When
Slack delivery is uncertain, inspect the actual conversation before manually
reposting a retained result. Raw results remain in the surface database and
kernel history. Tokens are not stored in the surface database or logged.

## Protocol references

- [Socket Mode](https://docs.slack.dev/apis/events-api/using-socket-mode/)
- [App manifests](https://docs.slack.dev/reference/app-manifest/)
- [Posting messages](https://docs.slack.dev/reference/methods/chat.postMessage/)
- [Updating messages](https://docs.slack.dev/reference/methods/chat.update/)

The adapter uses existing Rust HTTP, WebSocket, and SQLite dependencies.

## Routine result delivery

The separate `renoa-host` service owns routine scheduling and execution. Slack only
projects completed Host results into the specialist's ready channel. Schema 6 adds
a delivery cursor and durable outbox: admission and cursor advancement commit
together, posting intent precedes the Slack call, and unreceipted posts remain
unknown after restart. `inspect` exposes recent routine delivery states. A missing
channel binding leaves that bot's result waiting without blocking other bots;
reconnecting Slack drains retained Host results.
See the Host architecture document for routine timing, management, and launch settings.
