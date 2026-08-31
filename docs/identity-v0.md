# Renoa device identity v0

> This is the current trust binding for the RCP proof. The protocol's canonical
> role and authorization boundaries are defined in [rcp-v0.md](rcp-v0.md).
> Its concrete enrollment and authentication frames are documented in
> [rcp-json-ws-v0.md](rcp-json-ws-v0.md).

## Outcome

A client cannot choose who it is. A trusted coordinator caller enrolls one
device for one exact peer identity, and every later connection derives its
principal and role from that durable device record.

This closes two separate holes: authenticating the socket and authorizing the
task. A valid device still cannot attach to or submit work for a task owned by a
different principal.

Each installation receives its own credential. Continuing the same task from a
phone, desktop, or integration authenticates that installation independently;
no device credential is handed to another device.

## Vocabulary

- A **principal** owns tasks.
- A **device** is one installation with one revocable credential.
- A **peer identity** binds that device to either a surface and principal or an
  execution node.
- An **enrollment** is a short-lived, single-use authority to create a device.
- A **session** is one temporary authenticated connection. A device can have
  more than one session.

## Enrollment and connection flow

1. Trusted administration calls `Coordinator::create_enrollment` with the exact
   peer identity and an expiry.
2. Renoa generates a 256-bit random enrollment token and stores only a
   domain-separated SHA-256 digest.
3. The device sends the token as its first WebSocket message. Tokens are never
   accepted in a URL.
4. One atomic SQLite transaction verifies expiry, consumes the enrollment, and
   creates the device.
5. The server returns a separate 256-bit device credential once and stores only
   its digest.
6. A fresh connection presents the device ID and credential. The server loads
   the peer identity; the client sends no principal, surface, or node claim.
7. Revocation is persisted before every active session for that device is
   cancelled. Connection registration and revocation share one lifecycle
   boundary so an authentication already in flight cannot restore a revoked
   executor.

## Invariants

1. Enrollment tokens and device credentials come from the operating system's
   cryptographically secure random source.
2. Enrollment tokens expire and can be consumed once.
3. Enrollment and credential plaintext are never stored by the coordinator.
4. Enrollment and credential digests use separate domains, so the two secret
   types cannot be substituted.
5. Authentication failures do not reveal whether a device is missing, revoked,
   expired, or carrying the wrong credential.
6. Peer identity is selected before enrollment and cannot be changed during
   authentication.
7. Task attachment and command submission both enforce task ownership.
8. Revocation terminates current sessions and rejects future sessions.
9. Enrollment creates a new device credential. Importing or cloning an existing
   device credential is not a surface-handoff mechanism.
10. Device credentials, task cursors, command outboxes, and provider or tool
    credentials remain separate state with separate owners.

The schema is versioned. A database created before task ownership existed is
rejected with an explicit error because assigning owners automatically would
turn a migration into an authorization decision.

## Production enrollment direction

The self-hosted coordinator has one stable HTTPS origin. On an interactive
device, the person authenticates at that origin with WebAuthn and authorizes a
new device whose role and principal are selected by the coordinator. WebAuthn
proves the person; the resulting Renoa device identity authenticates later RCP
connections. They are not the same credential.

For a browser surface, the long-lived Renoa device credential must remain out
of JavaScript storage. An authenticated same-origin HTTPS session obtains a
short-lived, single-use WebSocket connection ticket instead. Native surfaces
and nodes store their device credential in the platform keychain, keystore, or
service-manager credential facility. An owner-only service credential file is
the explicit fallback for a headless system without one of those facilities.

A headless node may request enrollment, but an existing trusted device must
approve the exact node identity before the coordinator creates its enrollment.
Typed device codes and QR scanning are fallbacks for constrained devices, not
the normal phone or browser flow. First-device bootstrap and total account
recovery remain explicit local administrative ceremonies; they must not create
an unauthenticated remote enrollment path.

These are selected product and security boundaries. Their HTTP/WebAuthn and
connection-ticket messages are not part of JSON/WebSocket binding version 8
and must not be added until an implementation and conformance test consume
them.

## Security boundary

The v0 credential is a bearer secret. Whoever steals it can impersonate that
device. Production clients must keep it in an operating-system credential
store, and production traffic must use WSS/TLS. The coordinator continues to
reject non-loopback listeners; the first public deployment terminates TLS in an
outbound tunnel before forwarding to that loopback listener.

Bearer credentials are the smallest complete proof for a personal runtime. A
future threat model may justify sender-constrained credentials through a
standard such as mTLS or DPoP. Renoa does not invent an application signing
protocol in v0.

`create_enrollment` and `revoke_device` are currently trusted administrative
Rust APIs, not remotely callable protocol messages. Credential rotation,
account recovery, per-source rate limiting, and the selected production
enrollment flow remain implementation work. Public TLS termination is proven;
human-facing device management is not.

The transport constraints follow the
[OWASP WebSocket Security guidance](https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html).
Interactive enrollment follows
[WebAuthn](https://www.w3.org/TR/webauthn-3/) and the
[IETF cross-device-flow security guidance](https://www.rfc-editor.org/rfc/rfc10027.html),
which favors same-device or phishing-resistant authentication over copied
device codes when capable devices are available.
The bearer-secret limitation is the one defined by
[RFC 6750](https://www.rfc-editor.org/rfc/rfc6750), while the possible future
sender-constrained direction is described in
[RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html#name-sender-constrained-access-t).
