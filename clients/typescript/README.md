# TypeScript RCP surface client

This is Renoa's first non-Rust RCP implementation. It is a private shared
transport core with a Node reference wrapper and a browser entry point, not a
stable public SDK and not an agent harness.

It currently owns the surface-side continuity mechanics:

- version 9 device or one-use browser-ticket authentication and task discovery;
- replay followed by live task events;
- a caller-owned durable cursor committed only after the surface callback succeeds;
- a caller-owned durable command outbox written before transmission;
- exact command retry after an uncertain acknowledgement;
- fully decoded baseline task activity with stable command-to-execution
  causation;
- typed RCP errors and observable disconnect reasons;
- reattachment of completed in-memory subscriptions after a host-triggered
  reconnect.

The Node wrapper uses SQLite. The browser Control Room imports
`@renoa/rcp-client/browser` and supplies an IndexedDB state implementation.
The host either supplies device credentials from its platform keychain or a
callback that obtains a fresh one-use browser ticket. The shared transport does
not persist either credential. Command input in the outbox remains private
surface data.

```ts
import { RcpSurfaceClient } from "@renoa/rcp-client";

const client = new RcpSurfaceClient({
  endpoint: "ws://127.0.0.1:8080/connect",
  authentication: { type: "device", credentials },
  statePath: "/private/device-state/rcp.sqlite",
});

await client.connect();
const tasks = await client.listTasks();
await client.attach(tasks[0].taskId, async (event) => {
  await projection.apply(event);
});

const submission = await client.submitText(tasks[0].taskId, "continue the work");
await submission.accepted;
```

A browser surface uses `authentication: { type: "ticket", getTicket }`.
`getTicket` must complete the passkey HTTP flow and return a new ticket for
each connection attempt; a consumed ticket cannot reconnect.

`submission.commandId` exists after the local outbox commit.
`submission.accepted` resolves only after coordinator admission. If it rejects
because the outcome is uncertain, reconnect and call
`retryPendingCommands()`. The same command identity and content are reused.
Pending commands are retried in durable insertion order, stopping at the first
rejection; retry scheduling remains host policy.

`connect()` restores attachments registered on the same client instance. After
a process restart, the host creates a client, registers each desired attachment
again, and the durable cursor selects the missing suffix. Retry timing remains
host policy; the client does not run an unbounded reconnect loop.

`waitForDisconnect()` returns the exact error that ended the current
connection. A `replay_required` error means the host should reconnect and
attach again; an attachment interrupted before replay completed is not retained
as active.

Requires Node 24 or newer. Build and run the cross-language tests with:

```sh
pnpm install --frozen-lockfile
pnpm test
```
