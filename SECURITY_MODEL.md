# Renoa threat model

Scope: whole `Anrahya/Renoa` repository. Infrastructure: `deploy/` (systemd units, `renoa-deploy`, `release.json`). The owner confirmed there is no separate infrastructure repository. Cloudflare Tunnel, Tailscale and VPS host configuration are not in the repository.

## System overview and trust boundaries

Renoa is a **single-owner** personal agent system (owner: Yash). One VPS runs every service under hardened systemd units (`deploy/*.service`): the coordinator, node and chat surfaces as the `renoa` user, the registry and `cloudflared` as `DynamicUser` accounts.

- **Internet → Cloudflare Tunnel → coordinator (loopback :7818).** This is the only public origin, `https://renoa.live`. It is the main trust boundary.
- **Tailnet → registry (:7820) and site gateway.** Both bind to loopback and are reached only through the owner's Tailscale Serve. No sign-in is needed on the site gateway.
- **Coordinator ↔ node.** The node connects out to the coordinator as an enrolled device and executes tasks with the shared Host.
- **Host → agent tools.** The agent's `bash`, file and Code Mode tools run unsandboxed as `renoa` in the workspace, by design. Live apps run inside bwrap and slirp4netns (`crates/renoa-local/src/apps/sandbox.rs`), with private, CGNAT and link-local ranges blocked.
- **Chat surfaces → Host.** Telegram, Slack and Discord connect out to their providers' APIs and open no listener.

## Components

| Component | Code | Notes |
|---|---|---|
| Coordinator | `crates/renoa-control` (bin `renoa-coordinator`) | Public HTTP/WS: `/connect`, `/v1/identity/*`, `/v1/host/*` relay, OAuth relay and callback, credential relay and setup page, device requests, `/s/{share}`, asset links, Control Room static files. Owns identity, devices, tasks, journal and push. |
| Node | `crates/renoa-node`, `nodes/` | Execution bridge, automations, Host control, site gateway (`site_gateway/`, including the app reverse proxy). |
| Host | `crates/renoa-local`, `renoa-kernel`, `renoa-agent*`, `renoa-runtime`, `renoa-store-sqlite`, `renoa-home`, `renoa-core` | Agent composition, tools (bash, file tools), MCP client and OAuth, plugins, apps sandbox, sites, memory, credentials storage. |
| Surfaces | `crates/renoa-telegram`, `renoa-slack`, `renoa-discord`, `renoa-acp`, `surfaces/control-room` | Chat and browser surfaces. |
| Registry | `crates/renoa-registry`, `renoa-registry-protocol` | Immutable Agent Plugin packages, reached over the tailnet only. |
| Clients | `clients/android`, `clients/mac`, `clients/typescript`, `renoa-rcp-client`, `renoa-rcp-wire` | Native apps holding device keys and credentials. |
| Adapters | `adapters/*`, `plugins/*` | Node.js model-provider, MCP client, MCP registry and Google Drive processes. |
| Deploy | `deploy/` | Release install and verification (`renoa-deploy`, SHA256SUMS), unit hardening. |

## Entry points and who can reach them

- **Anyone on the internet:** every coordinator route via renoa.live, plus the Control Room public homepage. The unauthenticated routes are pairing and passkey ceremonies, device requests, the OAuth provider callback, credential relay pages, share links (`/s/{share}`, two-minute one-use ticket), asset links, Host owner discovery (`/v1/host/access`, returns the sole node owner's principal id) and OAuth client metadata (`/v1/oauth/client-metadata.json`).
- **Owner devices only** (enrolled device credential, key-bound app, passkey-backed remembered browser cookie, 60-second connection ticket): `/connect` RCP operations and the `/v1/host` relay.
- **Owner's tailnet:** the registry and the site gateway (`/sites/<name>/`, `/apps/<name>/`).
- **Chat platforms:** Telegram (one `RENOA_TELEGRAM_ALLOWED_USER_ID`, private chat only, `ingress.rs`), Slack (one `allowed_user_id`), and Discord (operator plus **guests** in bound channels).
- **Third parties whose responses the system consumes:** model providers, MCP servers, OAuth authorization servers, the Telegram, Slack and Discord APIs, FCM, and the official MCP Registry.

## Assets (owner-ranked)

1. **Owner identity** (most important, per the owner): device credentials and device keys, passkeys, remembered browser sessions, connection tickets and enrollments. Losing any of these gives an attacker full control of the owner's Host.
2. **Shell access on the VPS** (most important): code execution as `renoa` reaches every Host database, every credential and every agent.
3. **Provider API keys and OAuth tokens** (most important): stored under `~/.renoa/credentials/`, in MCP OAuth secrets (`crates/renoa-local/src/mcp/oauth/secret.rs`), and in systemd `LoadCredential` files under `/etc/renoa/`.
4. Conversation history, agent memory and the task journal. These are sensitive, but the owner did not rank them as top priority.
5. Release integrity: `renoa-deploy`, plus the APK and Mac zip served from the Control Room `app/` directory. The owner did not rank this as top priority.

## Attacker profiles that matter

The owner said only the single owner can get a principal or account; nobody else can sign up. So the attackers are:

- An **anonymous internet client** hitting renoa.live: authentication or pairing bypass, ticket or share-link guessing or replay, CSRF or cross-origin use of the owner's cookie, OAuth-relay abuse, and DoS.
- A **Discord guest** or a non-allowlisted Telegram or Slack user trying to obtain tool execution or owner data.
- A **malicious or compromised third-party response** (model provider, MCP server, OAuth server, chat API) that yields code execution, a credential leak or a sandbox escape. This does not include steering the owner's own turns.
- A **holder of one copied bearer credential**, for example a leaked device credential or cookie. Key-bound devices must not accept the bearer alone.
- A **tailnet peer or local user** for the registry and site gateway. This is a weaker attacker; the precondition must be noted.

A "second principal" or tenant attacker does not exist in this deployment. Cross-principal isolation still matters as defense in depth, but only an owner-controlled identity could exploit it.

## Existing controls

- Identity design (`docs/identity-v0.md`): secrets are stored only as domain-separated SHA-256 digests (`crates/renoa-control/src/identity.rs`). Tickets are one-use and short-lived. Key-bound devices prove a P-256 key on every connection.
- WebAuthn through `webauthn-rs` 0.5.5 with RP origin validation (`browser_identity.rs::validate_origin`). Allowed app origins are explicit `android:apk-key-hash:` values.
- Browser login is an HTTP-only, secure, same-site cookie. `renoa-deploy` health checks expect 401 for an unauthenticated Host request and 403 for a foreign origin.
- Guest turns get no tools: `crates/renoa-local/src/speaker.rs` returns `ToolAccess::Refused` and `GUEST_REFUSAL`.
- Telegram and Slack accept exactly one allowlisted user ID.
- Systemd hardening: `NoNewPrivileges`, `ProtectSystem=strict`, an empty `CapabilityBoundingSet`, `UMask=0077`, secrets passed through `LoadCredential`, and `IPAddressDeny=any` on the registry.
- The apps sandbox uses bwrap and slirp4netns with blocked private ranges. The site gateway serves snapshots with `nosniff` on an origin that holds no Renoa credential.
- Releases are verified against SHA256SUMS and per-binary manifest digests. The Monty worker binary hash is pinned.

## Security invariants

- **Authentication (CWE-287, CWE-294, CWE-613).** Every coordinator route except the public ceremony, relay, share and asset routes must resolve a `PeerIdentity` before any RCP or Host operation. Bearer secrets (enrollments, pairing codes, connection tickets, share tickets) are random values stored only as digests, expiry-checked server-side and consumed atomically in SQLite; tickets are single-use. Device-request codes are eight Crockford base32 characters (`device_requests.rs`), stored as a digest. OAuth and credential relays are stored by their relay ID, with separate state and capability digests (`control_schema.rs`). A key-bound device must fail to connect with its bearer credential alone (`device_key`, `device_handshake.rs`). WebAuthn ceremony state lives only on the server. The `danger-allow-state-serialisation` feature must never let a client supply or replay ceremony state.
- **Authorization and access control (CWE-639, CWE-862).** Task access checks the task's owning principal against the connection's `PeerIdentity` (`task_opening.rs`, `task_deletion.rs`, `command_stop.rs`, `node_ownership`). Node-only messages are accepted only from execution-node identities, and surface operations only from surface identities. `/v1/host` relays only for the authenticated owner and checks Origin.
- **Guest tool isolation (CWE-863).** A `Speaker::Guest` turn (Discord `Author::Guest`) runs with `ToolAccess::Refused` in `speaker.rs`; Code Mode, MCP, automations and plugins must not be reachable from it. `Speaker::Unidentified` keeps every tool: it is used for Telegram and Slack turns (`renoa-telegram/src/service/execution.rs`, `renoa-slack/src/worker.rs`) and for another agent's message (`renoa-node/src/origin.rs`). For Telegram and Slack, the ingress checks are therefore what keep outsiders from tools: Telegram accepts only the allowlisted user's private chat (`renoa-telegram/src/ingress.rs`), and Slack accepts only the configured workspace, bot installation and user (`renoa-slack/src/ingress.rs` `parse`). Surfaces must derive `Author` from the platform sender ID, never from message text.
- **CSRF and cross-origin (CWE-352, CWE-1385).** State-changing HTTP routes and the `/connect` WebSocket upgrade must reject foreign `Origin`s. The cookie stays HttpOnly, Secure and SameSite. Credential setup and share pages must not reflect attacker-controlled HTML.
- **SSRF, open redirect and OAuth (CWE-918, CWE-601).** The OAuth relay uses one fixed `oauth_callback_uri` and binds each callback to a relay created by an authenticated device (`DEVICE_ID_HEADER`), with bounded query sizes. Callback results go only to that device. MCP and model-adapter outbound URLs come from owner configuration. Live apps cannot reach private, CGNAT or link-local ranges (`BLOCKED_RANGES`).
- **Injection (CWE-89, CWE-78).** SQLite access uses `rusqlite` `params!`. The `format!` SQL in `renoa-local` (memory jobs, automations, catalog cutover) interpolates only compile-time constants and table names, never input. Agent `bash` running as `renoa` is intended behaviour; the invariant is that no non-owner input reaches `Command::new` outside an owner turn.
- **Path traversal and file handling (CWE-22, CWE-59).** File tools resolve paths under the workspace root (`file_tools.rs` `writable_path` and `existing_file`). Site snapshots reject `..`, root and prefix components and canonicalize them inside the workspace (`sites/snapshot.rs`). The site gateway and share pages serve only snapshot or shared files, with `Content-Disposition` and `nosniff` set. Symlinks must not escape. The registry refuses a symlinked home.
- **Secrets (CWE-312, CWE-532, CWE-200).** Device credentials and provider keys or tokens never enter logs, task journals, live activity, share links or model context. A share link carries only its own one-use, two-minute share ticket in the URL (`share_links.rs`). Units load them through `LoadCredential` with `UMask=0077`. The coordinator does not persist `/v1/host` request or response bodies.
- **Sandbox escape (CWE-693).** Supervised apps get only the bwrap namespaces and binds in `apps/sandbox.rs`. They hold no Renoa credentials, and the site gateway's proxy (`site_gateway/proxy.rs`, `ws.rs`) forwards only to supervised app sockets.
- **Supply chain and release (CWE-494).** `renoa-deploy install <tag>` checks the downloaded archive and apps against the release's SHA256SUMS. Every install, including a local `--archive`, checks each binary against the archive's bundled manifest; a local archive is not checked against SHA256SUMS. Apps served from `app/` match SHA256SUMS. `cargo-deny` (`deny.toml`) gates dependencies. Gap: SHA256SUMS comes from the same GitHub release, so integrity rests on the GitHub account rather than an independent signature.

## Out of scope and accepted risk (owner decisions)

- **Prompt injection of the owner's own turns** (web pages, MCP results, files, guest text the agent reads) that leads to tool misuse is accepted risk. The owner trusts the agent's unsandboxed shell. Only report guest or outsider paths that **themselves** get a tool to run.
- The owner's agent having full shell and file access as `renoa` is accepted by design.
- Multi-tenant, public sign-up and second-principal attackers do not apply: the system has a single owner.

## Open questions

- Cloudflare Tunnel ingress rules and Tailscale ACLs are not in the repository. Their exposure is assumed from `deploy/README.md`.
- No independent release signing exists beyond GitHub-hosted SHA256SUMS.
