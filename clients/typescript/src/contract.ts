import {
  uuid,
  type DeviceCredentials,
  type LiveActivity,
  type RcpErrorCode,
  type TaskEvent,
} from "./protocol.js";

// The client's public contract: what a host supplies and what it receives.

type Awaitable<T> = T | Promise<T>;

export interface PendingTextCommand {
  readonly commandId: string;
  readonly taskId: string;
  readonly text: string;
}

export interface RcpSurfaceState {
  cursor(taskId: string): Awaitable<number | null>;
  advanceCursor(taskId: string, sequence: number): Awaitable<void>;
  enqueueCommand(command: PendingTextCommand): Awaitable<void>;
  pendingCommands(): Awaitable<readonly PendingTextCommand[]>;
  removeCommand(commandId: string): Awaitable<void>;
  close(): Awaitable<void>;
}

export type RcpAuthentication =
  | { readonly type: "device"; readonly credentials: DeviceCredentials }
  | { readonly type: "ticket"; readonly getTicket: () => string | Promise<string> };

export interface RcpSurfaceClientCoreOptions {
  readonly endpoint: string;
  readonly authentication: RcpAuthentication;
  readonly state: RcpSurfaceState;
}

export type ApplyTaskEvent = (event: TaskEvent) => void | Promise<void>;

/**
 * Receives the live activity of the task's running turns. Frames are advisory:
 * they may be missed, are never replayed, and are superseded by the durable
 * task events that carry the same text. The client does not wait for what this
 * returns and ignores a throw or a rejection, so a surface's trouble drawing a
 * draft neither ends the connection nor delays task events. Because it is not
 * awaited, an async callback that must keep frames in order has to queue them
 * itself.
 */
export type ApplyLiveActivity = (
  commandId: string,
  activity: LiveActivity,
) => void | Promise<void>;

/** Hands one live frame to the surface's callback; see {@link ApplyLiveActivity}. */
export function deliverLiveActivity(
  live: ApplyLiveActivity,
  commandId: string,
  activity: LiveActivity,
): void {
  try {
    Promise.resolve(live(commandId, activity)).catch(() => undefined);
  } catch {
    // The surface's drawing of a draft is its own concern.
  }
}

export interface CommandSubmission {
  readonly commandId: string;
  readonly accepted: Promise<void>;
}

export class RcpError extends Error {
  readonly code: RcpErrorCode;
  readonly requestId: number | null;

  constructor(code: RcpErrorCode, message: string, requestId: number | null) {
    super(message);
    this.name = "RcpError";
    this.code = code;
    this.requestId = requestId;
  }
}

export function assertRcpConnectionOptions(
  endpoint: string,
  authentication: RcpAuthentication,
): void {
  if (typeof endpoint !== "string" || endpoint === "") {
    throw new Error("endpoint must be a non-empty string");
  }
  if (authentication.type === "device") {
    uuid(authentication.credentials.deviceId, "credentials.deviceId");
    if (
      typeof authentication.credentials.credential !== "string" ||
      authentication.credentials.credential === ""
    ) {
      throw new Error("credentials.credential must be a non-empty string");
    }
    return;
  }
  if (typeof authentication.getTicket !== "function") {
    throw new Error("authentication.getTicket must be a function");
  }
}
