# Arcee Telegram surface

`renoa-telegram` is the first hosted surface for Arcee, Renoa's personal operator.
It is a thin Telegram Bot API adapter around the ordinary `renoa-local` Host:
the Host assembles Arcee from the profile, model, loop, skills, and tools, while
the kernel remains the only owner of Agent history and execution state.

Each private Telegram topic has one current Renoa session. `/new` moves only
that topic to a fresh session; earlier admitted work retains its original
session. `/compact`, `/status`, `/model`, `/reasoning`, and `/cancel` operate on
the topic's current session. `/model` refreshes OpenCode Go's authenticated
catalog before listing choices; `/model <id>` and `/reasoning <level>` change
only future operations. Only private updates whose chat and sender both equal
the configured user ID are admitted.

The surface journals every accepted Telegram update before advancing its
`getUpdates` offset. Request, session, and draft identities remain stable over
restart. Model execution can be retried safely by Renoa. Telegram sends cannot:
if the process dies while a final message is in flight, delivery is recorded as
unknown and is not blindly repeated.

Streaming uses Telegram's native `sendRichMessageDraft`: short assistant
progress survives model/tool round trips in a bounded thinking block while the
current answer streams separately. Updates reuse one stable draft identity and
are locally paced below Telegram's published limits. Final answers use rich
messages with a plain-text fallback. Hidden model reasoning is never shown.
The first version accepts text messages only and executes one turn at a time
across the surface.

## Configuration

Build the model and optional MCP adapters as described in
[`renoa-local`](../renoa-local/README.md), then set:

```sh
export RENOA_DATA_DIR='/absolute/path/to/private/renoa-data'
export RENOA_TELEGRAM_WORKSPACE='/absolute/path/to/arcee-workspace'
export RENOA_TELEGRAM_ALLOWED_USER_ID='123456789'
export RENOA_TELEGRAM_BOT_TOKEN_FILE='/absolute/path/to/owner-only/token-file'
export RENOA_TELEGRAM_IPV4_ONLY='1' # Only when this Host has a broken IPv6 route.
export RENOA_MODEL_BRIDGE='/absolute/path/to/adapters/model-provider-node/dist/src/main.js'
export RENOA_MODEL_AUTH_STORE='/absolute/path/to/model-auth.sqlite'
export RENOA_MODEL_PROVIDER='opencode-go'
export RENOA_MODEL='your-model-id'
export TZ='Asia/Kolkata' # Optional; otherwise use the host system time zone.

# Optional Host pieces:
export RENOA_MODEL_PROVIDERS='opencode-go'
export RENOA_MCP_ADAPTER='/absolute/path/to/adapters/mcp-client-node/dist/src/main.js'
export RENOA_MCP_REGISTRY_ADAPTER='/absolute/path/to/adapters/mcp-registry-node/dist/src/main.js'
export RENOA_SHARED_PLUGIN_REGISTRY='http://tailnet-host:8082/'
export RENOA_OAUTH_RELAY_ORIGIN='https://renoa.live'
export RENOA_OAUTH_RELAY_DEVICE_CREDENTIAL_FILE='/absolute/path/to/owner-only/node-device.json'
```

Arcee's Host profile permits only OpenCode Go. The model adapter remains a
replaceable Host component shared with other profiles; Telegram contains no
provider-specific request code. A future Discord surface can call the same
session configuration methods without moving model state into Discord.

The two OAuth relay settings are atomic: set both or neither. With them, Arcee
shows a provider authorization link in Telegram instead of trying to open a
browser on the VPS. The callback returns through the configured HTTPS origin;
OAuth client state and tokens remain in Arcee's private Host directory. The
device file authenticates only relay management and should be supplied through
the service manager's credential facility in production.
The same settings enable secure intake when an MCP needs an API token or a
pre-registered OAuth client. Arcee sends a permanent setup button; the browser
encrypts the value for this Host, and the connection becomes active only after
the Host stores the credential and successfully discovers the MCP tools.

The token file must contain exactly one Bot API token. Do not put that token in
an environment file or command line. Arcee refuses to start when the bot has a
webhook configured because long polling and webhooks are mutually exclusive;
remove a previous webhook deliberately before restarting it. Enable private
chat topics in BotFather if independent topic sessions are wanted.
Address-family selection defaults to normal dual-stack networking. Setting
`RENOA_TELEGRAM_IPV4_ONLY=1` binds only Telegram API connections to IPv4; this
does not restrict Agent tools or MCP connections.

Run locally with:

```sh
cargo run --locked -p renoa-telegram
```

The service emits one JSON record per line on stderr for journald or another
log collector. It excludes prompts, model content, and credentials.
