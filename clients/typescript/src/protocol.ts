export const RCP_VERSION = 7;

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export interface DeviceCredentials {
  readonly deviceId: string;
  readonly credential: string;
}

export interface TaskSummary {
  readonly taskId: string;
  readonly target: string;
}

export interface TaskEvent {
  readonly eventId: string;
  readonly taskId: string;
  readonly sequence: number;
  readonly kind: Readonly<Record<string, unknown>>;
}

export interface RcpSurfaceClientOptions {
  readonly endpoint: string;
  readonly credentials: DeviceCredentials;
  readonly statePath: string;
}

export type RcpErrorCode =
  | "authentication_failed"
  | "invalid_message"
  | "invalid_role"
  | "node_offline"
  | "not_found"
  | "conflict"
  | "internal"
  | "replay_required"
  | "version_mismatch";

export type ServerMessage =
  | { readonly type: "authenticated"; readonly version: number }
  | {
      readonly type: "task_list";
      readonly request_id: number;
      readonly tasks: readonly TaskSummary[];
    }
  | {
      readonly type: "attached";
      readonly request_id: number;
      readonly task_id: string;
      readonly through_sequence: number | null;
    }
  | {
      readonly type: "command_accepted";
      readonly request_id: number;
      readonly command_id: string;
    }
  | { readonly type: "task_event"; readonly event: TaskEvent }
  | {
      readonly type: "error";
      readonly request_id: number | null;
      readonly code: RcpErrorCode;
      readonly message: string;
    };

export function parseServerMessage(json: string): ServerMessage {
  const value: unknown = JSON.parse(json);
  const object = record(value, "server message");
  const type = string(object.type, "message type");
  switch (type) {
    case "authenticated":
      return {
        type,
        version: safeUnsignedInteger(object.version, "version"),
      };
    case "task_list":
      return {
        type,
        request_id: safeUnsignedInteger(object.request_id, "request_id"),
        tasks: array(object.tasks, "tasks").map((task) => parseTask(task)),
      };
    case "attached":
      return {
        type,
        request_id: safeUnsignedInteger(object.request_id, "request_id"),
        task_id: uuid(object.task_id, "task_id"),
        through_sequence:
          object.through_sequence === null
            ? null
            : safeUnsignedInteger(object.through_sequence, "through_sequence"),
      };
    case "command_accepted":
      return {
        type,
        request_id: safeUnsignedInteger(object.request_id, "request_id"),
        command_id: uuid(object.command_id, "command_id"),
      };
    case "task_event":
      return { type, event: parseTaskEvent(object.event) };
    case "error":
      return {
        type,
        request_id:
          object.request_id === null
            ? null
            : safeUnsignedInteger(object.request_id, "request_id"),
        code: errorCode(object.code),
        message: string(object.message, "error message"),
      };
    default:
      throw new Error(`unsupported RCP message type ${type}`);
  }
}

function parseTaskEvent(value: unknown): TaskEvent {
  const event = record(value, "task event");
  const kind = record(event.kind, "task event kind");
  string(kind.type, "task event kind type");
  return {
    eventId: uuid(event.eventId, "eventId"),
    taskId: uuid(event.taskId, "taskId"),
    sequence: safeUnsignedInteger(event.sequence, "sequence"),
    kind,
  };
}

function parseTask(value: unknown): TaskSummary {
  const task = record(value, "task summary");
  return {
    taskId: uuid(task.taskId, "taskId"),
    target: string(task.target, "target"),
  };
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${name} must be an object`);
  }
  return value as Record<string, unknown>;
}

function array(value: unknown, name: string): readonly unknown[] {
  if (!Array.isArray(value)) {
    throw new Error(`${name} must be an array`);
  }
  return value;
}

function string(value: unknown, name: string): string {
  if (typeof value !== "string") {
    throw new Error(`${name} must be a string`);
  }
  return value;
}

export function uuid(value: unknown, name: string): string {
  const candidate = string(value, name);
  if (!UUID_PATTERN.test(candidate)) {
    throw new Error(`${name} must be a canonical UUID`);
  }
  return candidate;
}

function safeUnsignedInteger(value: unknown, name: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw new Error(`${name} must be a safe unsigned integer`);
  }
  return value as number;
}

function errorCode(value: unknown): RcpErrorCode {
  const code = string(value, "error code");
  switch (code) {
    case "authentication_failed":
    case "invalid_message":
    case "invalid_role":
    case "node_offline":
    case "not_found":
    case "conflict":
    case "internal":
    case "replay_required":
    case "version_mismatch":
      return code;
    default:
      throw new Error(`unsupported RCP error code ${code}`);
  }
}
