export const RCP_VERSION = 9;

export interface DeviceCredentials {
  readonly deviceId: string;
  readonly credential: string;
}

export interface ExecuteCommand {
  readonly taskId: string;
  readonly commandId: string;
  readonly principalId: string;
  readonly surface: string;
  readonly target: string;
  readonly text: string;
}

export interface QueuedExecution extends ExecuteCommand {
  readonly executionId: string;
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

export type ServerMessage =
  | { readonly type: "authenticated"; readonly version: number }
  | { readonly type: "execute"; readonly command: ExecuteCommand }
  | { readonly type: "execution_acknowledged"; readonly commandId: string }
  | {
      readonly type: "execution_events_accepted";
      readonly commandId: string;
      readonly throughSequence: number;
    }
  | {
      readonly type: "error";
      readonly code: string;
      readonly message: string;
    };

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export function parseServerMessage(json: string): ServerMessage {
  const message = record(JSON.parse(json), "server message");
  const type = string(message.type, "message type");
  switch (type) {
    case "authenticated":
      return {
        type,
        version: safeUnsignedInteger(message.version, "version"),
      };
    case "execute":
      return { type, command: parseExecute(message) };
    case "execution_acknowledged":
      return {
        type,
        commandId: uuid(message.command_id, "command_id"),
      };
    case "execution_events_accepted":
      return {
        type,
        commandId: uuid(message.command_id, "command_id"),
        throughSequence: safeUnsignedInteger(
          message.through_execution_sequence,
          "through_execution_sequence",
        ),
      };
    case "error":
      return {
        type,
        code: string(message.code, "error code"),
        message: string(message.message, "error message"),
      };
    default:
      throw new Error(`unsupported RCP node message ${type}`);
  }
}

function parseExecute(message: Record<string, unknown>): ExecuteCommand {
  const command = record(message.command, "execute command");
  const input = record(command.input, "command input");
  if (string(input.type, "command input type") !== "text") {
    throw new Error("Pi node only accepts text commands");
  }
  return {
    taskId: uuid(message.task_id, "task_id"),
    commandId: uuid(command.commandId, "commandId"),
    principalId: uuid(command.principalId, "principalId"),
    surface: string(command.surface, "surface"),
    target: string(command.target, "target"),
    text: string(input.text, "command text"),
  };
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${name} must be an object`);
  }
  return value as Record<string, unknown>;
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
