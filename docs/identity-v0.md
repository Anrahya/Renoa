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

A signed-in app is a third path: a passkey proves the person once, and a key
generated on the device, whose private half never leaves it, proves the
installation on every connection. A copied bearer credential alone is not a
device.

A computer without the passkey may be signed in by such an app instead: the
owner types the code the computer shows into the app, and the computer's own
key proves it from then on.

A valid identity still cannot access a task owned by another principal. Device
authentication, person authentication, and task authorization are separate
checks.

## Vocabulary

- A **principal** owns tasks.
- A **device** is one native installation or node with one revocable credential.
- A **device key** is a P-256 key pair an app generates on its device. The
  coordinator stores only the public key. A device that has registered one is
  **key-bound**: its bearer credential is not enough to connect.
- A **sign-in** is a passkey ceremony that also registers a device key and ends
  in a device credential instead of a browser ticket.
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
- A **device request** is a computer's five-minute ask to be signed in by
  one of its owner's apps, identified by a code the computer shows.
- A **connection ticket** is a 60-second, one-use browser bearer secret bound to
  one surface peer identity.
- A **share ticket** is a two-minute, one-use bearer secret that opens one file
  an agent shared, as one principal, once.
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

"Same-origin" below is one rule: the request names exactly one `Origin` header
and it equals the configured origin. Pairing, logout, transport tickets and
every Host write (`/v1/host` reads need only the cookie) refuse a request
without it. The four passkey endpoints also accept a request with no `Origin`,
for native clients, but refuse one that names another origin or repeats it.

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
`revoke-browser-logins <database> <principal-id>` atomically revokes all remembered
logins and existing pairing codes for that principal, including codes not yet
claimed. Redemption and revocation serialize through the same SQLite write
transaction: an earlier redemption loses its session, and a later redemption
finds no grant. Another principal is unaffected. Trusted local administration
can issue new codes afterward for recovery. Existing WebSockets/native device
credentials and registered passkeys are separate.
Storage outages remain errors rather than being classified as invalid credentials.

## App sign-in and device keys

A signed-in app proves the person with a passkey and the installation with a
device key. Nothing here is specific to one platform: an app names its own
`surface`, and the contract is the same for Android, Mac, Windows or Linux.

### The key

The app generates a P-256 key pair on the device (on Android, in the Keystore)
and registers the public key as the 65-byte uncompressed point (`0x04`, X, Y),
base64url without padding. A signature is ECDSA P-256 over SHA-256 in ASN.1 DER
form, base64url without padding, which is what Android's `SHA256withECDSA`
and Apple's `ecdsaSignatureMessageX962SHA256` produce. The coordinator uses the
key to verify signatures only. Whether the same key also receives sealed data is
for the work that seals to it (#104), not assumed here. A reconnect from the
background has no person to unlock the phone, so the app must create the key
without a per-use authentication requirement.

Two messages are signed, both UTF-8 text with `\n` separators and lowercase
hyphenated UUIDs:

```text
renoa device registration v1\n<ceremonyId>
renoa device proof v1\n<deviceId>\n<nonce>
```

A checked-in vector lets a client prove it builds the same messages and
signatures (`crates/renoa-control/src/device_key/tests.rs`). The public key is
`BEFC5QCFE_LZheDMR2lWsqSFcKXGmLzushLPtXThJVeqASyH5pnjQFelgVnW4rcU5iGrCF77DrbWES30UVNgswU`.
The proof for device `00000000-0000-0000-0000-0000000000d1` and nonce
`0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef` is
`MEUCIAnitA2CIfhmPWOB6eOIiJM_Z74hBIIyaP9nxLfrHoibAiEAlm4ly-JV0fkETPZSIdyS8Q0Xk9NkXWmvGYLnnjViCeA`.
The registration proof for ceremony `00000000-0000-0000-0000-0000000000c1` is
`MEUCIGTBknvC6AVKrU-9KczMBrhfuFPvQsETvh3tXWR-Jm5_AiEAw054Y_wN-AB-Iw5JllHACXM-4__CGQLWZn6FoU6ccj0`.
ECDSA signatures are randomized, so a client checks a signature by verifying it,
not by comparing bytes.

### Signing in

Sign-in uses the four passkey endpoints above. The options request of either
ceremony takes an optional `device`:

```json
{ "principalId": "...", "surface": "android",
  "device": { "publicKey": "<point>", "label": "Pixel 9" } }
```

A registration request carries `bootstrapToken` instead of `principalId`, for a
person whose passkey is not synced to the phone. `label` is the name the owner
will recognize, 1 to 64 printable characters. The coordinator stores the key
with the ceremony, so the assertion is bound to a key chosen before it was made.
The verify request adds `deviceProof`, the key's signature over the registration
message. It is required for a ceremony that carries a device and refused for one
that does not. The response of such a ceremony is
`{ "credentials": { "deviceId", "credential" } }`, with no ticket and no
cookie. One transaction records the passkey change and the device, and the
credential's digest, before the response is sent.

The key is the stable identity the app owns before it first sends. A sign-in
with the same principal, surface and key adopts the existing device, replaces its
credential and updates its label, and ends the device's live connections before
the response returns the new credential, so a lost response or a lost credential
never duplicates a device. The replaced credential is not accepted again: a
connection that authenticated with it and was still proving its key when the new
credential committed is refused when it would become active, because activation
checks, under the connection lifecycle lock, that the device is not revoked and
still holds the credential the connection presented. A key that is revoked, or
registered to another principal or surface, is a `conflict`: revocation is never
undone by replaying a sign-in. A principal may hold 32 active key-bound devices.
The transaction enforces these rules for both ceremonies. A registration
request, which already holds the bootstrap token, also checks them at the options
request and says why, so a refused sign-in leaves the one-use token unspent. An
authentication options request carries no secret, so it answers the same for
every key: the refusal waits for the passkey assertion at verify, names its
cause then, and writes nothing, so the passkey keeps its counter and the app
signs in again with a new key. A ceremony claimed and then refused is not
replayed.

### Signing in a computer from a phone

A computer without the owner's passkey signs in through an app that already
proved its key. The computer makes a device key as an app does and chooses a
random request id. It sends

```json
{ "requestId": "...", "surface": "mac",
  "device": { "publicKey": "<point>", "label": "MacBook" },
  "deviceProof": "<signature>" }
```

to `POST /v1/identity/device-requests`, where `deviceProof` is the key's
signature over

```text
renoa device request v1\n<requestId>
```

For the key
`BCTt5QvEGs7ZC__8uHrpq0TMB1vN4N6Oj_uXPaVnRW2X9YNJbnBWYVPO8JdOMT0-GBJ_96iwoJe8_rAx6S-Fbx0`
and request `00000000-0000-0000-0000-0000000000a1`, one such signature is
`MEQCIGDfIpht28Svysr4vkpKODTzftc77LblqNZ-tPqZYCTCAiA_sM32e6lIMZEHAqB3H6drSMOfDWHDDDeRUKXD5rYBaA`
(checked in `crates/renoa-control/src/device_requests/tests.rs`).

Every check (surface, key, label, proof) runs before anything is stored. The
answer is `{ "code", "expiresAtMs" }`: eight symbols of Crockford's base32
(40 bits), which the computer shows, and when the request ends, five minutes
after it was first made. The coordinator stores only the code's digest. Asking
again with the same id, key, surface and label replaces the code and keeps the
expiry, so a lost answer converges on one request; another key or name under
that id is a `conflict`, as is asking again once the request was approved.

The owner types the code into a signed-in app, which sends
[`approve_device`](rcp-json-ws-v0.md#sign-in-a-computer). Only a connection
that proved its key may: a browser ticket connection outlives the session that
issued it. Approval names the app's principal and the approving device, and
runs the checks a sign-in runs (a revoked key, a key of another sign-in, the
device limit), refusing without approving, so a refused approval leaves the
computer waiting. Another principal's approval of an approved request is a
`conflict`; the same principal's is acknowledged again and names the newer
approving device. Revoking the approving device withdraws every approval it
gave that no computer has outlived: those requests wait again.

A code that matches no waiting request counts against the principal that sent
it. After five such misses within five minutes, every approval from that
principal, the right code included, is a `conflict` until the oldest miss is
five minutes old.

The computer collects at `POST /v1/identity/device-requests/collect` with
`{ "requestId", "deviceProof" }`, the same signature. A request no app has
approved answers `202` with `{ "expiresAtMs" }`; an approved one answers `200`
with `{ "credentials": { "deviceId", "credential" } }`, creating or adopting the
device exactly as a sign-in does in one transaction. That transaction runs the
sign-in checks again, so a key revoked or signed in elsewhere since the
approval is a `conflict` and stores nothing. Each collection until the request
expires issues a new credential for the same device and ends the connections
that used the old one, so a lost answer is collected again. A proof the
request's key did not make is `authentication_failed`; an expired or unknown
request is `not_found`. The computer then connects as any key-bound device.

At most 64 requests wait at once, the bound passkey ceremonies have; another is
refused with `capacity_exceeded` until one expires. Only an app that proved its
key may try a code, so with at most 64 live a guess finds one with a chance of
2^-34, and a principal makes at most five wrong guesses in five minutes.

What this does not stop:

- An owner who types a code someone else's computer shows signs that computer
  in as themselves. The app says to enter only a code from a computer in front
  of the owner; nothing on the coordinator can tell the two apart.
- The proof is the same for every collection of one request. Whoever learns it
  can collect, but the credential they receive only works with the computer's
  key, so a replay costs the computer its current credential and nothing more.
- Opening a request checks the proof, not the sign-in rules: a revoked key may
  wait for a code, and approval refuses it.
- Anyone may hold the 64 waiting slots, which delays computer sign-in until the
  requests expire. Passkey sign-in and connected devices are unaffected.

### Connecting

`authenticate` for a key-bound device is answered with `device_challenge`
carrying a fresh 256-bit nonce, kept only in that connection's memory. The
client answers `device_proof` with its signature over the proof message and
receives `authenticated`. A missing, malformed, foreign or recorded proof ends
the connection with `authentication_failed` and reaches no operation. Devices
without a key, which are nodes and service links, authenticate as before.

### Administering devices

A connection established by a browser ticket or by a proven device key may list
and revoke the principal's key-bound devices (`list_devices`, `revoke_device`).
A service link that presented only its bearer credential may not, so a chat
surface cannot cut off the owner's apps. Revocation commits before it is
acknowledged and then ends the device's sessions; a device may revoke itself, and
its acknowledgement is sent before its socket closes. A retry succeeds again.
Every revocation, the owner's and the operator's, deletes the device's push
token in the transaction that marks it revoked, and the push dispatcher starts
sends only to devices that are not revoked, so no send starts to a revoked
device. A send already started or handed to FCM when the revocation lands cannot
be recalled; it carries no content.

A connection by a proven device key, and no other, may call the owner's Host
over RCP (`call_host`). That is how an app controls the Host: its credential
opens no `/v1/host` route, which takes only a browser's session cookie. A
browser's ticket connection may not, because it outlives the session that
issued it; revoking a device ends its connections.

The same connection, and no other, may ask for a share ticket (`share_link`),
because a ticket reads the Host as its principal. The coordinator stores its
digest with the principal and the share for two minutes. A `GET /s/{share}`
with it reads the file's description and first part as that principal, and
only then deletes the row, so one request alone is served; a ticket for another
share is not used up, and an expired row is dropped when the next ticket is
issued. The ticket travels in the link's query, so it is
single-use and short-lived instead of secret for long. Revoking the device does
not withdraw a ticket it already holds: that ticket still opens its one file
until it is used or expires.

### Android apps

Android shows a passkey to an app only when the relying party lists the app, and
an assertion made by an app carries the origin
`android:apk-key-hash:<base64url SHA-256 of the signing certificate>`. The
coordinator reads `RENOA_ANDROID_APPS_FILE`, a JSON file of
`{"apps": [{"packageName": "...", "sha256CertFingerprints": ["14:6D:..."]}]}`
with fingerprints as `keytool` prints them (one per build signature, such as a
debug and a release certificate). Each fingerprint becomes an allowed origin
matched by exact equality, and `/.well-known/assetlinks.json` is served with
`delegate_permission/common.handle_all_urls` and
`delegate_permission/common.get_login_creds` for the same package and
fingerprints, the relations Google's Credential Manager prerequisites list.
`handle_all_urls` also verifies any App Links a listed app declares for this
host. Unset means neither: no app origin is trusted and no file is served. An
invalid file stops `serve` at startup. This is relying-party configuration; no
RCP frame names a platform.

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
8. Device revocation ends current device sessions, rejects future ones, and
   deletes the device's push token.
9. Identity credentials, task cursors, command outboxes, provider credentials,
   and tool credentials remain separate state with separate owners.
10. A database predating task ownership is rejected; a migration cannot invent
    authorization.
11. Reusing device authentication on an auxiliary HTTP route preserves the
    enrolled server-side role and grants no implicit RCP task authority.
12. A key-bound device reaches no operation on its bearer credential alone: its
    connection must sign a fresh, per-connection challenge with the registered
    key. The coordinator holds no device's private key.
13. A sign-in records the passkey change and creates or adopts the device in one
    transaction, and a key that was revoked is never revived by a later sign-in.
14. Only a browser ticket or a proven device key may list or revoke devices. A
    bearer-only link may not.
15. Auxiliary HTTP routes accept only node identities, which are never key-bound,
    so a key-bound device's bearer credential opens none of them.
16. A computer is signed in by a code only for the principal of an app that
    proved its key, under the rules of a sign-in checked at approval and again
    at collection; only the holder of the request's key collects its
    credential, and revoking the approving app withdraws approvals no
    computer has collected.

## Remaining work

The browser Control Room implements pairing, passkey registration and
authentication, and remembered logins. It requests a fresh ticket for every
connection attempt and keeps that ticket in memory only. Signed-in apps have
sender-constrained device credentials and owner-side device listing and
revocation. A computer signs in by a code a signed-in app approves. Nodes and
service links such as Discord remain bearer-only, with credentials in
owner-only files. Approval for headless node enrollment, passkey
administration, recovery, credential rotation other than
a repeat sign-in, per-source throttling, monitoring, and backup restoration
remain outside this foundation slice.

Renoa uses [`webauthn-rs` 0.5.5](https://github.com/kanidm/webauthn-rs/releases/tag/v0.5.5)
under MPL-2.0 for WebAuthn validation and its test authenticator. Renoa enables
serialization only for server-side SQLite ceremony state; it does not adapt or
copy upstream source. The security rules follow
[WebAuthn Level 3](https://www.w3.org/TR/webauthn-3/) and the
[OWASP WebSocket Security guidance](https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html).
