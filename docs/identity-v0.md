# Renoa identity v0

> This is the canonical identity architecture for the RCP coordinator. Protocol
> authorization lives in [rcp-v0.md](rcp-v0.md); concrete WebSocket frames live
> in [rcp-json-ws-v0.md](rcp-json-ws-v0.md).

## Outcome

A client cannot choose who it is. Native surfaces and nodes authenticate as a
durably enrolled device. A browser pairs through a locally issued owner code or
proves the person with a passkey. Both establish a remembered browser login that
can issue a short-lived ticket bound to one principal and surface.
Both paths establish the same `PeerIdentity` before any RCP operation runs.

A valid identity still cannot access a task owned by another principal. Device
authentication, person authentication, and task authorization are separate
checks.

## Vocabulary

- A **principal** owns tasks.
- A **device** is one native installation or node with one revocable credential.
- A **peer identity** binds a connection to either a principal and surface or an
  execution node.
- An **enrollment** is a one-use authority to create a native device.
- A **passkey bootstrap** is a local, one-use authority to register the first or
  another passkey for one exact principal.
- A **browser pairing** is a local authority to admit one remembered browser for
  one exact principal, without requiring a passkey provider.
- A **remembered browser login** is a revocable, hashed bearer credential carried
  only as a secure HTTP-only same-site cookie after admission.
- A **ceremony** is one WebAuthn registration or authentication attempt whose
  challenge state exists only on the server.
- A **connection ticket** is a 60-second, one-use browser bearer secret bound to
  one surface peer identity.
- A **session** is one temporary authenticated WebSocket connection.

## Native device flow

1. Trusted administration calls `Coordinator::create_enrollment` with the exact
   peer identity and an expiry.
2. Renoa stores only a domain-separated SHA-256 digest of the random 256-bit
   enrollment token.
3. The device sends the token as its first WebSocket frame. One SQLite
   transaction verifies expiry, consumes it, creates a device, and stores only
   the digest of a separate random 256-bit credential.
4. Later connections present the device ID and credential. The coordinator
   loads the peer identity; the client sends no principal or role claim.
5. Revocation is committed before active sessions are cancelled. Authentication
   and revocation share a lifecycle boundary, so an in-flight connection cannot
   restore a revoked executor.

Every installation has its own credential. Handoff never copies another
device's credential. Native clients keep it in a platform keychain, keystore,
or service-manager credential facility; an owner-only file is the explicit
headless fallback.

## Auxiliary HTTP authentication

One enrolled device credential may also authenticate a narrow coordinator HTTP
route owned by that installation. The request carries the device ID in
`X-Renoa-Device-Id` and the credential as an `Authorization: Bearer` value.
The coordinator resolves the stored peer identity exactly as it does for a
WebSocket connection; the request cannot claim or change its role.

The first consumer is the MCP OAuth callback-relay management API. Only a Node
identity may create, poll, or acknowledge its own relay records. This does not
turn the API into an RCP transport or grant task access. The provider-facing
callback is intentionally unauthenticated by device credential: its 256-bit
OAuth state is the single-use correlation secret, and the coordinator stores
only its digest before the callback arrives. Device credentials never appear in
URLs, callback pages, or OAuth provider traffic.

## Browser passkey flow

The coordinator is configured with one exact relying-party ID and origin. The
origin must be HTTPS, except HTTP is accepted for `localhost` tests. A local
administrator starts passkey registration with:

```text
renoa-coordinator bootstrap-passkey <database> <principal-id>
```

The command prints a 30-minute, one-use token. It is the only implemented
passkey-registration authority; no unauthenticated remote endpoint can create
one.

The same-origin browser then uses four JSON `POST` endpoints:

```text
/v1/identity/passkeys/registration/options
/v1/identity/passkeys/registration/verify
/v1/identity/passkeys/authentication/options
/v1/identity/passkeys/authentication/verify
```

Registration options accept `{ bootstrapToken, surface }`. The bootstrap binds
the principal; the browser cannot supply it. Authentication options accept
`{ principalId, surface }`; the principal ID is an opaque public identifier,
not a credential. An options response contains `{ ceremonyId, options }`. A
verify request contains `{ ceremonyId, credential }` and returns:

```json
{
  "connectionTicket": "<one-use secret>",
  "expiresAtMs": 1788278400000
}
```

Registration verification returns a ticket directly, avoiding a second biometric
prompt, and sets `__Host-renoa_session`. Authentication sets the same remembered
login. Passkey storage belongs to the authenticator, not Renoa; browsers without a
compatible OS provider, password manager, or security key can use direct pairing.

The browser sends the ticket in its first WebSocket frame. The coordinator
atomically deletes a valid ticket before replying `authenticated`. A lost reply
therefore requires a fresh ticket from the remembered login; ticket replay is never
treated as a reconnect mechanism. Once established, the WebSocket remains the
temporary session.

## Direct browser pairing and remembered login

Trusted local administration runs `renoa-coordinator pair-browser <database>
<principal-id>` against existing identity storage. It prints a random 256-bit code
with a 30-minute lifetime. No remote endpoint issues pairing codes. The code is a
bearer enrollment authority: possession permits one browser admission for its
server-bound principal. It is distinct from a passkey bootstrap and cannot be
substituted for native enrollment, passkey enrollment, or a transport ticket.

The same-origin browser posts `{ pairingToken, browserNonce }` to
`/v1/identity/pair`. It generates a random 256-bit nonce and retains the request in
memory until confirmation. The server derives a session secret with HMAC-SHA-256,
keyed by the pairing code over a domain-separated nonce. In one transaction it
claims the code for the resulting session digest and persists that session's
principal and expiry, then issues the cookie. Neither code, nonce, nor session
plaintext enters SQLite. There is no long-lived credential in JavaScript storage.

An identical retry before the code expires recovers the same cookie, including
after service restart or a lost response. Another nonce cannot reuse the code.
Retry checks the existing session rather than creating it again, so logout,
revocation, or session expiry cannot be undone by replay. Reloading the page before
confirmation loses its in-memory nonce and may require a fresh pairing code.

Both login methods use the same 180-day session lifetime, renewed after half its
lifetime during use. They have no IP or process-local secret binding. The identity
database stores the principal on each session; passkey-backed sessions additionally
reference their credential and stop working if that credential disappears. Schema
11 migrates existing schema-10 sessions without changing their cookies or owners.

`GET /v1/identity/session` validates and renews a login. Same-origin
`POST /v1/identity/connection-ticket` issues a fresh one-use transport ticket.
`POST /v1/identity/logout` revokes only the current browser, while local
`revoke-browser-logins <database> <principal-id>` revokes all remembered logins for
that principal. Existing WebSockets/native device credentials are separate.
Storage outages remain errors rather than being classified as invalid credentials.

## Durable state and failure rules

WebAuthn challenge state is stored in SQLite and never returned to the browser.
Registration and authentication ceremonies last five minutes and are claimed
once. Claim happens before cryptographic finishing. A coordinator crash after
claim but before commit fails closed: the person repeats authentication, or a
local administrator creates another registration bootstrap. Renoa never
replays an uncertain ceremony.

Registered passkeys store public credential data, their principal binding, and
the last observed nonzero authenticator counter. Authentication updates the
credential and issues its ticket in one transaction. A stale nonzero counter is
rejected, including when two ceremonies finish out of order. Authenticators
whose counters remain zero continue to work as WebAuthn permits.

Ticket and bootstrap plaintext never enter SQLite. Their 256-bit random values
use distinct domain-separated digests, so one secret type cannot substitute for
another. Expired bootstraps, ceremonies, and tickets are removed during identity
transactions.

The coordinator admits at most 64 active passkey ceremonies and at most 64 KiB
per identity request. Per-source throttling is still required before a
multi-user public service; this bound prevents unbounded durable growth today.

All identity JSON responses use `Cache-Control: no-store`, `Pragma: no-cache`,
`Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, and a deny-all
content security policy. Identity routes do not enable cross-origin requests.
Malformed input and authentication failures do not expose WebAuthn, credential,
or SQLite details.

## Invariants

1. Identity is established before an operation and cannot change on that
   connection.
2. Enrollment, bootstrap, device, and ticket secrets use the operating system's
   cryptographically secure random source and separate digest domains.
3. Enrollment tokens, bootstraps, ceremonies, and tickets expire and are
   consumed once. Pairing codes admit one browser with receipt-bound retries.
4. The server persists WebAuthn ceremony state; the client never receives it.
5. Passkey registration requires user verification and asserts that a
   credential ID is globally unique.
6. A browser ticket can establish only its server-bound surface identity and
   never creates a durable device row.
7. Task discovery, attachment, and submission enforce principal ownership after
   authentication.
8. Device revocation ends current device sessions and rejects future ones.
9. Identity credentials, task cursors, command outboxes, provider credentials,
   and tool credentials remain separate state with separate owners.
10. A database predating task ownership is rejected; a migration cannot invent
    authorization.
11. Reusing device authentication on an auxiliary HTTP route preserves the
    enrolled server-side role and grants no implicit RCP task authority.

## Remaining work

The browser Control Room implements pairing, passkey registration and
authentication, and remembered logins. It requests a fresh ticket for every
connection attempt and keeps that ticket in memory only. Trusted-device approval for
headless enrollment, device and passkey administration, recovery, per-source
throttling, monitoring, and backup restoration remain outside this foundation
slice.

Renoa uses [`webauthn-rs` 0.5.5](https://github.com/kanidm/webauthn-rs/releases/tag/v0.5.5)
under MPL-2.0 for WebAuthn validation and its test authenticator. Renoa enables
serialization only for server-side SQLite ceremony state; it does not adapt or
copy upstream source. The security rules follow
[WebAuthn Level 3](https://www.w3.org/TR/webauthn-3/) and the
[OWASP WebSocket Security guidance](https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html).
