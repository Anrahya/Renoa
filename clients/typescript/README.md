# TypeScript RCP surface client

This is Renoa's first non-Rust RCP implementation. It is a private, headless
Node reference client, not a stable public SDK and not an agent harness.

It currently owns the surface-side continuity mechanics:

- version 7 authentication and task discovery;
- replay followed by live task events;
- a SQLite cursor committed only after the surface callback succeeds;
- a SQLite command outbox written before transmission;
- exact command retry after an uncertain acknowledgement;
- typed RCP errors and observable disconnect reasons;
- reattachment of completed in-memory subscriptions after a host-triggered
  reconnect.

The host supplies device credentials and must keep them in its platform
keychain or keystore. The client does not put credentials in its SQLite file.
That file does contain plaintext command input, so it must remain private to the
device installation.

```ts
import { RcpSurfaceClient } from "@renoa/rcp-client";

const client = new RcpSurfaceClient({
  endpoint: "ws://127.0.0.1:8080/connect",
  credentials,
  statePath: "/private/device-state/rcp.sqlite",
});

await client.connect();
const tasks = await client.listTasks();
await client.attach(tasks[0].taskId, async (event) => {
  await projection.apply(event);
});

const submission = client.submitText(tasks[0].taskId, "continue the work");
await submission.accepted;
```

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
