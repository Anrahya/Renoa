import {
  RCP_VERSION,
  parseServerMessage,
  uuid,
  type DeviceCredentials,
  type RcpErrorCode,
  type ServerMessage,
  type TaskEvent,
  type TaskSummary,
} from "./protocol.js";

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

interface PendingRequest {
  complete(message: ServerMessage): void;
  reject(error: Error): void;
}

export type ApplyTaskEvent = (event: TaskEvent) => void | Promise<void>;

export interface CommandSubmission {
  readonly commandId: string;
  readonly accepted: Promise<void>;
}

interface Attachment {
  readonly taskId: string;
  readonly apply: ApplyTaskEvent;
  cursor: number | null;
  replay:
    | {
        readonly through: number;
        readonly resolve: () => void;
        readonly reject: (error: Error) => void;
      }
    | undefined;
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

export class RcpSurfaceClientCore {
  readonly #endpoint: string;
  readonly #identity: RcpAuthentication;
  readonly #state: RcpSurfaceState;
  readonly #pending = new Map<number, PendingRequest>();
  readonly #attachments = new Map<string, Attachment>();
  #socket: WebSocket | undefined;
  #handshake:
    | { readonly resolve: () => void; readonly reject: (error: Error) => void }
    | undefined;
  #disconnection:
    | { readonly promise: Promise<Error>; readonly resolve: (error: Error) => void }
    | undefined;
  #nextRequestId = 1;
  #authenticated = false;
  #connecting = false;
  #closed = false;
  #receiving = Promise.resolve();

  constructor(options: RcpSurfaceClientCoreOptions) {
    assertRcpConnectionOptions(options.endpoint, options.authentication);
    if (options.authentication.type === "device") {
      this.#identity = {
        type: "device",
        credentials: { ...options.authentication.credentials },
      };
    } else {
      this.#identity = options.authentication;
    }
    this.#endpoint = options.endpoint;
    this.#state = options.state;
  }

  async connect(): Promise<void> {
    if (this.#closed) {
      throw new Error("RCP client is closed");
    }
    if (this.#socket !== undefined || this.#connecting) {
      throw new Error("RCP client is already connected");
    }
    this.#connecting = true;
    let identity: object;
    try {
      identity = await this.#identityMessage();
    } finally {
      this.#connecting = false;
    }
    if (this.#closed) {
      throw new Error("RCP client is closed");
    }
    const socket = new WebSocket(this.#endpoint);
    this.#disconnection = Promise.withResolvers<Error>();
    this.#socket = socket;
    socket.addEventListener("message", (event) => this.#enqueue(socket, event));
    socket.addEventListener("close", () =>
      this.#disconnect(socket, new Error("RCP connection closed")),
    );
    socket.addEventListener("error", () =>
      this.#disconnect(socket, new Error("RCP connection failed")),
    );

    try {
      await opened(socket);
      const authenticated = new Promise<void>((resolve, reject) => {
        this.#handshake = { resolve, reject };
      });
      socket.send(JSON.stringify(identity));
      await authenticated;
      for (const attachment of this.#attachments.values()) {
        await this.#attachExisting(attachment);
      }
    } catch (error) {
      this.#disconnect(socket, asError(error));
      socket.close();
      throw error;
    }
  }

  async disconnect(): Promise<void> {
    const socket = this.#socket;
    if (socket === undefined) {
      return;
    }
    if (socket.readyState === WebSocket.CLOSED) {
      this.#disconnect(socket, new Error("RCP connection closed"));
      return;
    }
    const closed = new Promise<void>((resolve) => {
      socket.addEventListener("close", () => resolve(), { once: true });
    });
    socket.close(1000);
    await closed;
  }

  waitForDisconnect(): Promise<Error> {
    if (this.#disconnection === undefined) {
      throw new Error("RCP client has not connected");
    }
    return this.#disconnection.promise;
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    await this.disconnect();
    await this.#receiving;
    await this.#state.close();
    this.#closed = true;
  }

  listTasks(): Promise<readonly TaskSummary[]> {
    return this.#request(
      (requestId) => ({ type: "list_tasks", request_id: requestId }),
      (message) => {
        if (message.type !== "task_list") {
          throw new Error(`expected task_list, received ${message.type}`);
        }
        return message.tasks;
      },
    );
  }

  async attach(taskId: string, apply: ApplyTaskEvent): Promise<void> {
    uuid(taskId, "taskId");
    this.#connectedSocket();
    if (this.#attachments.has(taskId)) {
      throw new Error(`task ${taskId} is already attached`);
    }
    const attachment: Attachment = {
      taskId,
      apply,
      cursor: await this.#state.cursor(taskId),
      replay: undefined,
    };
    this.#attachments.set(taskId, attachment);
    try {
      await this.#attachExisting(attachment);
    } catch (error) {
      this.#attachments.delete(taskId);
      throw error;
    }
  }

  async submitText(taskId: string, text: string): Promise<CommandSubmission> {
    uuid(taskId, "taskId");
    if (typeof text !== "string") {
      throw new Error("text must be a string");
    }
    this.#connectedSocket();
    const command: PendingTextCommand = {
      commandId: globalThis.crypto.randomUUID(),
      taskId,
      text,
    };
    await this.#state.enqueueCommand(command);
    const accepted = this.#sendPendingCommand(command);
    return { commandId: command.commandId, accepted };
  }

  async retryPendingCommands(): Promise<readonly string[]> {
    this.#connectedSocket();
    const accepted: string[] = [];
    for (const command of await this.#state.pendingCommands()) {
      await this.#sendPendingCommand(command);
      accepted.push(command.commandId);
    }
    return accepted;
  }

  async #sendPendingCommand(command: PendingTextCommand): Promise<void> {
    await this.#request(
      (requestId) => ({
        type: "submit",
        request_id: requestId,
        task_id: command.taskId,
        command_id: command.commandId,
        input: { type: "text", text: command.text },
      }),
      (message) => {
        if (message.type !== "command_accepted") {
          throw new Error(`expected command_accepted, received ${message.type}`);
        }
        if (message.command_id !== command.commandId) {
          throw new Error("command admission changed the command identity");
        }
      },
    );
    await this.#state.removeCommand(command.commandId);
  }

  async #attachExisting(attachment: Attachment): Promise<void> {
    attachment.cursor = await this.#state.cursor(attachment.taskId);
    await this.#request(
      (requestId) => ({
        type: "attach",
        request_id: requestId,
        task_id: attachment.taskId,
        after_sequence: attachment.cursor,
      }),
      (message) => {
        if (message.type !== "attached") {
          throw new Error(`expected attached, received ${message.type}`);
        }
        if (message.task_id !== attachment.taskId) {
          throw new Error("attach response changed the task identity");
        }
        return this.#waitForReplay(attachment, message.through_sequence);
      },
    );
  }

  #waitForReplay(attachment: Attachment, through: number | null): Promise<void> {
    if (through === null || (attachment.cursor !== null && attachment.cursor >= through)) {
      return Promise.resolve();
    }
    return new Promise((resolve, reject) => {
      attachment.replay = { through, resolve, reject };
    });
  }

  #request<T>(
    createMessage: (requestId: number) => object,
    accept: (message: ServerMessage) => T | PromiseLike<T>,
  ): Promise<T> {
    const socket = this.#connectedSocket();
    const requestId = this.#allocateRequestId();
    return new Promise<T>((resolve, reject) => {
      this.#pending.set(requestId, {
        complete: (message) => resolve(accept(message)),
        reject,
      });
      try {
        socket.send(JSON.stringify(createMessage(requestId)));
      } catch (error) {
        this.#pending.delete(requestId);
        reject(asError(error));
      }
    });
  }

  #enqueue(socket: WebSocket, event: MessageEvent): void {
    this.#receiving = this.#receiving
      .then(() => {
        if (this.#socket === socket) {
          return this.#receive(event);
        }
        return undefined;
      })
      .catch((error: unknown) => {
        const failure = asError(error);
        this.#disconnect(socket, failure);
        socket.close(4002, "invalid RCP message");
      });
  }

  async #receive(event: MessageEvent): Promise<void> {
    if (typeof event.data !== "string") {
      throw new Error("RCP server sent a non-text frame");
    }
    const message = parseServerMessage(event.data);
    if (message.type === "authenticated") {
      if (message.version !== RCP_VERSION || this.#handshake === undefined) {
        throw new Error("unexpected authenticated message");
      }
      const authentication = this.#handshake;
      this.#handshake = undefined;
      this.#authenticated = true;
      authentication.resolve();
      return;
    }
    if (message.type === "task_event") {
      await this.#applyTaskEvent(message.event);
      return;
    }
    if (message.type === "error") {
      const error = new RcpError(message.code, message.message, message.request_id);
      if (message.request_id === null) {
        this.#handshake?.reject(error);
        this.#handshake = undefined;
        throw error;
      }
      const pending = this.#pending.get(message.request_id);
      if (pending === undefined) {
        throw new Error(`error references unknown request ${message.request_id}`);
      }
      this.#pending.delete(message.request_id);
      pending.reject(error);
      return;
    }
    const pending = this.#pending.get(message.request_id);
    if (pending === undefined) {
      throw new Error(`response references unknown request ${message.request_id}`);
    }
    this.#pending.delete(message.request_id);
    try {
      pending.complete(message);
    } catch (error) {
      pending.reject(asError(error));
      throw error;
    }
  }

  async #applyTaskEvent(event: TaskEvent): Promise<void> {
    const attachment = this.#attachments.get(event.taskId);
    if (attachment === undefined) {
      throw new Error(`received an event for unattached task ${event.taskId}`);
    }
    if (attachment.cursor !== null && event.sequence <= attachment.cursor) {
      return;
    }
    const expected = attachment.cursor === null ? 0 : attachment.cursor + 1;
    if (event.sequence !== expected) {
      throw new Error(
        `task ${event.taskId} event sequence ${event.sequence} follows ${attachment.cursor}`,
      );
    }
    await attachment.apply(event);
    await this.#state.advanceCursor(event.taskId, event.sequence);
    attachment.cursor = event.sequence;
    if (attachment.replay !== undefined && event.sequence >= attachment.replay.through) {
      const replay = attachment.replay;
      attachment.replay = undefined;
      replay.resolve();
    }
  }

  #disconnect(socket: WebSocket, error: Error): void {
    if (this.#socket !== socket) {
      return;
    }
    this.#handshake?.reject(error);
    this.#handshake = undefined;
    this.#authenticated = false;
    for (const pending of this.#pending.values()) {
      pending.reject(error);
    }
    this.#pending.clear();
    for (const attachment of this.#attachments.values()) {
      attachment.replay?.reject(error);
      attachment.replay = undefined;
    }
    this.#disconnection?.resolve(error);
    this.#socket = undefined;
  }

  #connectedSocket(): WebSocket {
    if (this.#socket?.readyState !== WebSocket.OPEN || !this.#authenticated) {
      throw new Error("RCP client is not connected");
    }
    return this.#socket;
  }

  #allocateRequestId(): number {
    if (this.#nextRequestId > Number.MAX_SAFE_INTEGER) {
      throw new Error("RCP request id space exhausted");
    }
    return this.#nextRequestId++;
  }

  async #identityMessage(): Promise<object> {
    if (this.#identity.type === "device") {
      return {
        type: "authenticate",
        version: RCP_VERSION,
        credentials: this.#identity.credentials,
      };
    }
    const ticket = await this.#identity.getTicket();
    if (typeof ticket !== "string" || !/^[0-9a-fA-F]{64}$/.test(ticket)) {
      throw new Error("connection ticket must be 64 hexadecimal characters");
    }
    return {
      type: "authenticate_ticket",
      version: RCP_VERSION,
      ticket,
    };
  }
}

function opened(socket: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    socket.addEventListener("open", () => resolve(), { once: true });
    socket.addEventListener("error", () => reject(new Error("RCP connection failed")), {
      once: true,
    });
  });
}

function asError(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value));
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
