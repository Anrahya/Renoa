# Renoa identity v0

> This is the canonical identity architecture for the RCP coordinator. Protocol
> authorization lives in [rcp-v0.md](rcp-v0.md); concrete WebSocket frames live
> in [rcp-json-ws-v0.md](rcp-json-ws-v0.md).

## Outcome

A client cannot choose who it is. Native surfaces and nodes authenticate as a
durably enrolled device. A browser proves the person with a passkey and receives
a short-lived ticket bound by the coordinator to one principal and surface.
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

The command prints a five-minute, one-use token. It is the only implemented
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

Registration verification returns a ticket directly, avoiding a second
biometric prompt. Later visits run passkey authentication to get another
ticket. There is no browser session cookie and no long-lived RCP credential in
JavaScript storage.

The browser sends the ticket in its first WebSocket frame. The coordinator
atomically deletes a valid ticket before replying `authenticated`. A lost reply
therefore requires another passkey authentication; ticket replay is never
treated as a reconnect mechanism. Once established, the WebSocket remains the
temporary session.

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
   consumed once.
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

The browser Control Room implements the baseline registration and
authentication ceremonies and requests a fresh ticket for every connection
attempt. It keeps that ticket in memory only. Trusted-device approval for
headless enrollment, device and passkey administration, recovery, per-source
throttling, monitoring, and backup restoration remain outside this foundation
slice.

Renoa uses [`webauthn-rs` 0.5.5](https://github.com/kanidm/webauthn-rs/releases/tag/v0.5.5)
under MPL-2.0 for WebAuthn validation and its test authenticator. Renoa enables
serialization only for server-side SQLite ceremony state; it does not adapt or
copy upstream source. The security rules follow
[WebAuthn Level 3](https://www.w3.org/TR/webauthn-3/) and the
[OWASP WebSocket Security guidance](https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html).
