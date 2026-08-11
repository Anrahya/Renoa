import type { DeviceCredentials, ServerMessage } from "./protocol.js";
import { parseServerMessage, RCP_VERSION } from "./protocol.js";
import { Pulse } from "./pulse.js";
import { NodeState } from "./state.js";

export class ProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ProtocolError";
  }
}

class ConnectionError extends Error {}

export async function reconnect(
  endpoint: string,
  credentials: DeviceCredentials,
  state: NodeState,
  pulse: Pulse,
  signal: AbortSignal,
): Promise<void> {
  while (!signal.aborted) {
    try {
      await serveSession(endpoint, credentials, state, pulse, signal);
    } catch (error) {
      if (signal.aborted) {
        return;
      }
      if (!(error instanceof ConnectionError)) {
        throw error;
      }
    }
    if (!signal.aborted) {
      try {
        await delay(250, signal);
      } catch {
        if (!signal.aborted) {
          throw new Error("RCP reconnect delay failed");
        }
      }
    }
  }
}

async function serveSession(
  endpoint: string,
  credentials: DeviceCredentials,
  state: NodeState,
  pulse: Pulse,
  signal: AbortSignal,
): Promise<void> {
  const socket = new WebSocket(endpoint);
  try {
    await opened(socket, signal);
    const authentication = nextMessage(socket, signal);
    socket.send(
      JSON.stringify({
        type: "authenticate",
        version: RCP_VERSION,
        credentials,
      }),
    );
    let response: ServerMessage;
    try {
      response = parseServerMessage(await authentication);
    } catch (error) {
      throw new ProtocolError(`invalid authentication response: ${asError(error).message}`);
    }
    if (response.type === "error") {
      throw new ProtocolError(
        `RCP authentication failed (${response.code}): ${response.message}`,
      );
    }
    if (response.type !== "authenticated" || response.version !== RCP_VERSION) {
      throw new ProtocolError("coordinator returned an incompatible authentication response");
    }
    await serveAuthenticated(socket, state, pulse, signal);
  } finally {
    if (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING) {
      socket.close();
    }
  }
}

async function serveAuthenticated(
  socket: WebSocket,
  state: NodeState,
  pulse: Pulse,
  signal: AbortSignal,
): Promise<void> {
  const admissions = new Set<string>();
  const eventBatches = new Map<string, number>();
  const closed = Promise.withResolvers<void>();
  const failed = Promise.withResolvers<never>();
  let chain = Promise.resolve();

  const sendPending = () => {
    for (const publication of state.pendingPublications()) {
      if (!publication.admissionAcked && !admissions.has(publication.commandId)) {
        send(socket, {
          type: "acknowledge_execution",
          task_id: publication.taskId,
          command_id: publication.commandId,
        });
        admissions.add(publication.commandId);
      }
      if (publication.events.length > 0 && !eventBatches.has(publication.commandId)) {
        const through = publication.events.at(-1)?.sequence;
        if (through === undefined) {
          throw new ProtocolError("non-empty event batch has no final sequence");
        }
        send(socket, {
          type: "publish_execution_events",
          task_id: publication.taskId,
          command_id: publication.commandId,
          events: publication.events,
        });
        eventBatches.set(publication.commandId, through);
      }
    }
  };
  const schedule = (operation: () => void) => {
    chain = chain
      .then(operation)
      .then(sendPending)
      .catch((error: unknown) => {
        failed.reject(
          error instanceof ProtocolError || error instanceof ConnectionError
            ? error
            : new ProtocolError(asError(error).message),
        );
        socket.close();
      });
  };
  const onMessage = (event: MessageEvent) => {
    schedule(() => {
      if (typeof event.data !== "string") {
        throw new ProtocolError("coordinator sent a non-text WebSocket message");
      }
      handleMessage(parseServerMessage(event.data), state, admissions, eventBatches);
    });
  };
  const onClose = () => closed.resolve();
  const onError = () => socket.close();
  const onAbort = () => socket.close();
  socket.addEventListener("message", onMessage);
  socket.addEventListener("close", onClose, { once: true });
  socket.addEventListener("error", onError);
  signal.addEventListener("abort", onAbort, { once: true });
  const unsubscribe = pulse.subscribe(() => schedule(() => {}));

  try {
    schedule(() => {});
    await Promise.race([closed.promise, failed.promise]);
  } finally {
    unsubscribe();
    socket.removeEventListener("message", onMessage);
    socket.removeEventListener("error", onError);
    signal.removeEventListener("abort", onAbort);
    await chain.catch(() => {});
  }
}

function handleMessage(
  message: ServerMessage,
  state: NodeState,
  admissions: Set<string>,
  eventBatches: Map<string, number>,
): void {
  switch (message.type) {
    case "execute":
      state.admit(message.command);
      break;
    case "execution_acknowledged":
      if (!admissions.delete(message.commandId)) {
        throw new ProtocolError(
          `unsolicited execution acknowledgement for command ${message.commandId}`,
        );
      }
      state.acknowledgeAdmission(message.commandId);
      break;
    case "execution_events_accepted": {
      const expected = eventBatches.get(message.commandId);
      if (expected === undefined || expected !== message.throughSequence) {
        throw new ProtocolError(
          `unexpected event cursor ${message.throughSequence} for command ${message.commandId}`,
        );
      }
      eventBatches.delete(message.commandId);
      state.advancePublication(message.commandId, message.throughSequence);
      break;
    }
    case "error":
      throw new ProtocolError(`coordinator rejected node message (${message.code}): ${message.message}`);
    case "authenticated":
      throw new ProtocolError("coordinator repeated authentication inside a node session");
  }
}

function send(socket: WebSocket, message: object): void {
  if (socket.readyState !== WebSocket.OPEN) {
    throw new ConnectionError("RCP connection closed before publication");
  }
  const json = JSON.stringify(message);
  try {
    socket.send(json);
  } catch (error) {
    throw new ConnectionError(`RCP publication failed: ${asError(error).message}`);
  }
}

function opened(socket: WebSocket, signal: AbortSignal): Promise<void> {
  if (signal.aborted) {
    return Promise.reject(signal.reason);
  }
  return new Promise((resolve, reject) => {
    const open = () => {
      cleanup();
      resolve();
    };
    const fail = () => {
      cleanup();
      reject(new ConnectionError("RCP connection failed"));
    };
    const abort = () => {
      cleanup();
      socket.close();
      reject(signal.reason);
    };
    const cleanup = () => {
      socket.removeEventListener("open", open);
      socket.removeEventListener("error", fail);
      socket.removeEventListener("close", fail);
      signal.removeEventListener("abort", abort);
    };
    socket.addEventListener("open", open, { once: true });
    socket.addEventListener("error", fail, { once: true });
    socket.addEventListener("close", fail, { once: true });
    signal.addEventListener("abort", abort, { once: true });
  });
}

function nextMessage(socket: WebSocket, signal: AbortSignal): Promise<string> {
  if (signal.aborted) {
    return Promise.reject(signal.reason);
  }
  return new Promise((resolve, reject) => {
    const message = (event: MessageEvent) => {
      cleanup();
      if (typeof event.data === "string") {
        resolve(event.data);
      } else {
        reject(new ProtocolError("coordinator sent a non-text WebSocket message"));
      }
    };
    const fail = () => {
      cleanup();
      reject(new ConnectionError("RCP connection closed before authentication"));
    };
    const abort = () => {
      cleanup();
      reject(signal.reason);
    };
    const cleanup = () => {
      socket.removeEventListener("message", message);
      socket.removeEventListener("close", fail);
      socket.removeEventListener("error", fail);
      signal.removeEventListener("abort", abort);
    };
    socket.addEventListener("message", message, { once: true });
    socket.addEventListener("close", fail, { once: true });
    socket.addEventListener("error", fail, { once: true });
    signal.addEventListener("abort", abort, { once: true });
  });
}

function delay(milliseconds: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) {
    return Promise.reject(signal.reason);
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", abort);
      resolve();
    }, milliseconds);
    const abort = () => {
      clearTimeout(timer);
      reject(signal.reason);
    };
    signal.addEventListener("abort", abort, { once: true });
  });
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}
