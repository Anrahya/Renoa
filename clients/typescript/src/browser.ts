export {
  RcpError,
  RcpSurfaceClientCore as RcpSurfaceClient,
} from "./core-client.js";
export type {
  ApplyTaskEvent,
  CommandSubmission,
  PendingTextCommand,
  RcpAuthentication,
  RcpSurfaceClientCoreOptions as RcpSurfaceClientOptions,
  RcpSurfaceState,
} from "./core-client.js";
export type {
  CommandEnvelope,
  DeviceCredentials,
  ExecutionEvent,
  ExecutionEventKind,
  ExecutionTerminal,
  RcpErrorCode,
  TaskEvent,
  TaskEventKind,
  TaskSummary,
  TextCommandInput,
} from "./protocol.js";
