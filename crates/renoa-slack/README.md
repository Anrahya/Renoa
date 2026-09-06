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
DM session. All belong to the configured durable Agent.

Commands are ordinary messages (not Slack slash commands):

- `!new`: start a fresh session in this conversation.
- `!status`: inspect session, model, and context usage.
- `!model [id]`: list models or select one.
- `!reasoning [level]`: list levels or change reasoning.
- `!compact`: explicitly compact the current session.
- `!cancel`: stop the earliest outstanding model turn in this conversation.
- `!help`: show the controls.

Arcee uses the existing personal-operator recipe and full access through its
configured tools and workspace. One worker serializes requests across this
surface and holds at most one live session handle. This slice does not create
specialist recipes, schedules, additional channel bindings, or cross-machine
execution migration. It uses normal Slack messaging and does not require Slack
AI or paid workflow features.

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
5. Build and launch:

   ```sh
   cargo build --release -p renoa-slack
   target/release/renoa-slack run /absolute/path/to/slack.json
   ```

For a user service, install the binary in `~/.local/bin`, save the config as
`~/.config/renoa/slack.json`, and install `deploy/renoa-slack.service` under
`~/.config/systemd/user/`. Enable with `systemctl --user enable --now renoa-slack`.
A VPS user service needs lingering enabled to remain alive after logout.

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
only the binding used by later admissions.

A restart requeues interrupted execution with its original session/request IDs
and original observation time. The real kernel replays completed outcomes and
reconciles unfinished operations. Infrastructure failure leaves the request
pending for restart; it is not converted to a false terminal outcome.

The initial progress message is receipted separately. Updates to its known Slack
timestamp are repeatable. Progress is bounded, refreshed at most every two
seconds, and joined before final delivery. Private reasoning and raw provider
payloads are not published. Extension authorization/setup links can appear in
progress only in DMs. Channel progress directs the operator to cancel and
continue account setup privately; setup URLs are never published into channels.

Final output commits before delivery and is split into Unicode-safe chunks.
Each new post is marked in flight before calling Slack. Rate limits honor
`Retry-After`. If a new post's outcome is ambiguous, it becomes `unknown` and
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
