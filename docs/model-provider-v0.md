# Renoa model-provider adapter

`@renoa/model-provider` is the Node process that translates Renoa's
provider-neutral model contract into xAI and OpenCode Go API calls.

It is not the kernel, the durable loop, compaction, tools, ACP, Waku, or RCP.
Those remain provider-neutral. A future adapter can replace this process if it
speaks the same stdin/stdout protocol and environment contract.

## What it owns

- xAI/Grok over OAuth
- OpenCode Go over its API key
- OpenAI Chat Completions, OpenAI Responses, and Anthropic Messages transports
- one retry policy
- classified, redacted provider failures
- SQLite credential storage compatible with the existing `pi-auth.sqlite` file

## What stays outside it

Provider, surface, and product policy. Session durability, tool execution,
compaction, ACP, Waku, and RCP. The Rust host launches this process for
`catalog`, `describe`, and `stream` only.

## Providers and transports

| Provider | Auth | Transports |
| --- | --- | --- |
| xAI | OAuth device flow | Chat Completions and Responses as advertised by the pinned catalog |
| OpenCode Go | API key | Official Go transports only: Responses, Chat Completions, Anthropic Messages |

OpenCode models are advertised only when an official transport is known and the
pinned catalog supplies limits, tool support, and reasoning metadata. Transport
is never inferred from a model name. Official-only models without that metadata
are omitted. `minimax-m2.7` and `qwen3.6-plus` use Anthropic Messages per
current OpenCode documentation; their binding IDs therefore differ from the
previous Pi completions bindings.

The catalog is the pinned JSON copied from Pi `v0.84.2`. It does not fetch
`pi.dev`. Existing sessions store `{provider, model, reasoning}` and re-resolve
the catalog on the next admitted turn. Frozen operations pin the adapter
recovery identity

```text
renoa-model-provider-node/v1/{provider}/{model}/{binding}/reasoning-{level}
```

An unfinished `pi/...` operation cannot execute through this adapter. There is
no compatibility mapping; the kernel fail-closes with a runtime mismatch.

Grok 4.6 advertises `low`, `medium`, `high`, and `xhigh` only. Those values are
sent as Chat Completions `reasoning_effort`. The adapter does not advertise
`off` or `minimal`.

## Retry

Maximum 3 total inference attempts. Exponential backoff with jitter. Honor
`Retry-After` as RFC 9110 delay-seconds or HTTP-date, capped at 60 seconds.
Retry connection establishment, 408, 429, and 5xx except 501. Never retry
ordinary 4xx. Allow one OAuth refresh-and-retry after a genuine expired-token
401, inside the same 3-attempt budget. Never retry after assistant text,
reasoning, or tool-call output has been exposed. Never retry a broken
successful stream merely because no visible text arrived. Cancellation aborts
the active request and any backoff immediately. Official SDKs run with
`maxRetries: 0`.

Retry attempts emit structured `retry_attempt` records on the adapter protocol
and into the durable runtime trace. They never enter model context.

## Errors

Every terminal failure preserves, when available: provider, model, category,
safe message, HTTP status, provider code, request ID, Retry-After, attempt
count, nested cause, a bounded redacted `provider_message`, and whether
inference is `known_not_started` or `unknown`.

Missing `inference_outcome` on the wire is malformed and treated as `unknown`.
It is never invented as `known_not_started`.

Failures before any request may have been transmitted can be
`known_not_started`. Once a request may have been sent, reset, timeout, and
cancellation are `unknown` unless the provider proves rejection with HTTP 4xx.
Retryability is separate from that durable certainty.

Categories: `authentication`, `rate_limited`, `invalid_request`,
`context_window_exceeded`, `network`, `timeout`, `provider_unavailable`,
`protocol`, `stream_interrupted`, `cancelled`, `unknown`.

The concise message reaches ACP. The full redacted diagnostic is written to
Renoa's runtime trace, not into model context. Redaction matches exact
normalized field and header names: credentials, tokens, cookies, secrets, and
signed URL query parameters are removed. Token-budget and rate-limit telemetry
such as `max_tokens`, `max_output_tokens`, `input_tokens`, and
`x-ratelimit-*` is preserved.

## Credentials

Node's built-in SQLite. Default path remains
`$HOME/.config/renoa/pi-auth.sqlite` so existing xAI OAuth and OpenCode keys
keep working. Files are created `0600`. Single-flight refresh holds a
`BEGIN IMMEDIATE` transaction in a dedicated sidecar SQLite database for the
duration of the token request. The sidecar is separate so credential reads and
writes remain available. Waiters never steal from a live holder: a paused
process keeps its transaction, while process death releases the OS lock without
a timeout. Waiters poll until the stored credential rotates or the lock is
free, then compare-and-store. A failed refresh leaves the last valid credential
untouched.

## Configure Renoa Alpha

```sh
pnpm --dir adapters/model-provider-node install --frozen-lockfile --ignore-scripts
pnpm --dir adapters/model-provider-node build
export RENOA_MODEL_BRIDGE="$PWD/adapters/model-provider-node/dist/src/main.js"
export RENOA_MODEL_AUTH_STORE="$HOME/.config/renoa/pi-auth.sqlite"
export RENOA_MODEL_PROVIDER=xai   # or opencode-go
export RENOA_MODEL=grok-4.6
export RENOA_MODEL_REASONING=high # optional
```

Authenticate with `pnpm --dir adapters/model-provider-node auth:xai` or
`auth:opencode-go`. The RCP Pi node still uses `RENOA_PI_*` harness settings
and is not this adapter.

A replacement adapter must implement `catalog`, `describe`, and `stream` over
newline JSON on stdout, consume the same `RENOA_MODEL_*` environment, and
preserve binding IDs as SHA-256 of the advertised model spec JSON.
