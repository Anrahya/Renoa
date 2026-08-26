export const RCP_VERSION = 8;

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export interface DeviceCredentials {
  readonly deviceId: string;
  readonly credential: string;
}

export interface TaskSummary {
  readonly taskId: string;
  readonly target: string;
}

export interface TextCommandInput {
  readonly type: "text";
  readonly text: string;
}

export interface CommandEnvelope {
  readonly commandId: string;
  readonly principalId: string;
  readonly surface: string;
  readonly target: string;
  readonly input: TextCommandInput;
}

export type ExecutionTerminal =
  | { readonly status: "completed" }
  | { readonly status: "failed"; readonly error: string }
  | { readonly status: "cancelled"; readonly reason: string };

export type ExecutionEventKind =
  | { readonly type: "execution_started" }
  | { readonly type: "turn_started" }
  | { readonly type: "assistant_message"; readonly text: string }
  | {
      readonly type: "tool_started";
      readonly call_id: string;
      readonly name: string;
      readonly arguments: unknown;
    }
  | {
      readonly type: "tool_finished";
      readonly call_id: string;
      readonly output: string;
      readonly is_error: boolean;
    }
  | { readonly type: "execution_terminated"; readonly terminal: ExecutionTerminal };

export interface ExecutionEvent {
  readonly eventId: string;
  readonly executionId: string;
  readonly sequence: number;
  readonly recordedAtMs: number;
  readonly kind: ExecutionEventKind;
}

export type TaskEventKind =
  | { readonly type: "command_submitted"; readonly command: CommandEnvelope }
  | {
      readonly type: "execution_event";
      readonly commandId: string;
      readonly event: ExecutionEvent;
    };

export interface TaskEvent {
  readonly eventId: string;
  readonly taskId: string;
  readonly sequence: number;
  readonly kind: TaskEventKind;
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
  return {
    eventId: uuid(event.eventId, "eventId"),
    taskId: uuid(event.taskId, "taskId"),
    sequence: safeUnsignedInteger(event.sequence, "sequence"),
    kind: parseTaskEventKind(event.kind),
  };
}

function parseTaskEventKind(value: unknown): TaskEventKind {
  const kind = record(value, "task event kind");
  const type = string(kind.type, "task event kind type");
  switch (type) {
    case "command_submitted":
      return { type, command: parseCommand(kind.command) };
    case "execution_event":
      return {
        type,
        commandId: uuid(kind.commandId, "execution event commandId"),
        event: parseExecutionEvent(kind.event),
      };
    default:
      throw new Error(`unsupported task event kind ${type}`);
  }
}

function parseCommand(value: unknown): CommandEnvelope {
  const command = record(value, "command");
  const input = record(command.input, "command input");
  const inputType = string(input.type, "command input type");
  if (inputType !== "text") {
    throw new Error(`unsupported command input type ${inputType}`);
  }
  return {
    commandId: uuid(command.commandId, "commandId"),
    principalId: uuid(command.principalId, "principalId"),
    surface: string(command.surface, "surface"),
    target: string(command.target, "target"),
    input: { type: inputType, text: string(input.text, "command text") },
  };
}

function parseExecutionEvent(value: unknown): ExecutionEvent {
  const event = record(value, "execution event");
  return {
    eventId: uuid(event.eventId, "execution eventId"),
    executionId: uuid(event.executionId, "executionId"),
    sequence: safeUnsignedInteger(event.sequence, "execution sequence"),
    recordedAtMs: safeInteger(event.recordedAtMs, "recordedAtMs"),
    kind: parseExecutionEventKind(event.kind),
  };
}

function parseExecutionEventKind(value: unknown): ExecutionEventKind {
  const kind = record(value, "execution event kind");
  const type = string(kind.type, "execution event kind type");
  switch (type) {
    case "execution_started":
    case "turn_started":
      return { type };
    case "assistant_message":
      return { type, text: string(kind.text, "assistant message text") };
    case "tool_started":
      return {
        type,
        call_id: string(kind.call_id, "tool call_id"),
        name: string(kind.name, "tool name"),
        arguments: required(kind, "arguments", "tool arguments"),
      };
    case "tool_finished":
      return {
        type,
        call_id: string(kind.call_id, "tool call_id"),
        output: string(kind.output, "tool output"),
        is_error: boolean(kind.is_error, "tool is_error"),
      };
    case "execution_terminated":
      return { type, terminal: parseExecutionTerminal(kind.terminal) };
    default:
      throw new Error(`unsupported execution event kind ${type}`);
  }
}

function parseExecutionTerminal(value: unknown): ExecutionTerminal {
  const terminal = record(value, "execution terminal");
  const status = string(terminal.status, "execution terminal status");
  switch (status) {
    case "completed":
      return { status };
    case "failed":
      return { status, error: string(terminal.error, "execution failure") };
    case "cancelled":
      return { status, reason: string(terminal.reason, "execution cancellation") };
    default:
      throw new Error(`unsupported execution terminal status ${status}`);
  }
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

function required(
  object: Readonly<Record<string, unknown>>,
  key: string,
  name: string,
): unknown {
  if (!Object.hasOwn(object, key)) {
    throw new Error(`${name} is required`);
  }
  return object[key];
}

function string(value: unknown, name: string): string {
  if (typeof value !== "string") {
    throw new Error(`${name} must be a string`);
  }
  return value;
}

function boolean(value: unknown, name: string): boolean {
  if (typeof value !== "boolean") {
    throw new Error(`${name} must be a boolean`);
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

function safeInteger(value: unknown, name: string): number {
  if (!Number.isSafeInteger(value)) {
    throw new Error(`${name} must be a safe integer`);
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
