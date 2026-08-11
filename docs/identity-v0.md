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

The schema is versioned. A database created before task ownership existed is
rejected with an explicit error because assigning owners automatically would
turn a migration into an authorization decision.

## Security boundary

The v0 credential is a bearer secret. Whoever steals it can impersonate that
device. Production clients must keep it in an operating-system credential
store, and production traffic must use WSS/TLS. The current server therefore
continues to reject non-loopback listeners.

Bearer credentials are the smallest complete proof for a personal runtime. A
future threat model may justify sender-constrained credentials through a
standard such as mTLS or DPoP. Renoa does not invent an application signing
protocol in v0.

`create_enrollment` and `revoke_device` are trusted administrative Rust APIs,
not remotely callable protocol messages. A first-device bootstrap flow, QR
rendering, credential rotation, account recovery, rate limiting, and public TLS
termination remain separate product and deployment work.

The transport constraints follow the
[OWASP WebSocket Security guidance](https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html).
The bearer-secret limitation is the one defined by
[RFC 6750](https://www.rfc-editor.org/rfc/rfc6750), while the possible future
sender-constrained direction is described in
[RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html#name-sender-constrained-access-t).
