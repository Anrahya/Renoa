export { RcpSurfaceClientCore as RcpSurfaceClient } from "./core-client.js";
export { RcpError } from "./contract.js";
export type {
  ApplyLiveActivity,
  ApplyTaskEvent,
  CommandSubmission,
  PendingTextCommand,
  RcpAuthentication,
  RcpSurfaceClientCoreOptions as RcpSurfaceClientOptions,
  RcpSurfaceState,
} from "./contract.js";
export type {
  CommandEnvelope,
  DeviceCredentials,
  ExecutionEvent,
  ExecutionEventKind,
  ExecutionTerminal,
  LiveActivity,
  RcpErrorCode,
  StopOutcome,
  TaskEvent,
  TaskEventKind,
  TaskStatus,
  TaskSummary,
  TextCommandInput,
} from "./protocol.js";
