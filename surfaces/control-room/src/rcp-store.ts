import type {
  PendingTextCommand,
  RcpSurfaceState,
  TaskEvent,
} from "@renoa/rcp-client/browser";

const DATABASE_VERSION = 1;
const CURSORS = "cursors";
const OUTBOX = "outbox";
const EVENTS = "events";

interface CursorRow {
  readonly taskId: string;
  readonly sequence: number;
}

interface OutboxRow extends PendingTextCommand {
  readonly ordinal?: number;
}

export class ControlRoomStore implements RcpSurfaceState {
  readonly #database: Promise<IDBDatabase>;
  #closed = false;

  constructor(name = "renoa-control-room") {
    this.#database = openDatabase(name);
  }

  async cursor(taskId: string): Promise<number | null> {
    const database = await this.#open();
    const transaction = database.transaction(CURSORS, "readonly");
    const [row] = await Promise.all([
      request<CursorRow | undefined>(transaction.objectStore(CURSORS).get(taskId)),
      transactionDone(transaction),
    ]);
    return row?.sequence ?? null;
  }

  async advanceCursor(taskId: string, sequence: number): Promise<void> {
    if (!Number.isSafeInteger(sequence) || sequence < 0) {
      throw new Error("cursor sequence must be a safe unsigned integer");
    }
    const database = await this.#open();
    const transaction = database.transaction(CURSORS, "readwrite");
    const completed = transactionDone(transaction);
    const store = transaction.objectStore(CURSORS);
    const current = store.get(taskId) as IDBRequest<CursorRow | undefined>;
    current.onsuccess = () => {
      if (current.result === undefined || current.result.sequence < sequence) {
        store.put({ taskId, sequence } satisfies CursorRow);
      }
    };
    await completed;
  }

  async enqueueCommand(command: PendingTextCommand): Promise<void> {
    const database = await this.#open();
    const transaction = database.transaction(OUTBOX, "readwrite");
    const completed = transactionDone(transaction);
    transaction.objectStore(OUTBOX).add(command);
    await completed;
  }

  async pendingCommands(): Promise<readonly PendingTextCommand[]> {
    const database = await this.#open();
    const transaction = database.transaction(OUTBOX, "readonly");
    const [rows] = await Promise.all([
      request<OutboxRow[]>(transaction.objectStore(OUTBOX).getAll()),
      transactionDone(transaction),
    ]);
    return rows.map(({ commandId, taskId, text }) => ({ commandId, taskId, text }));
  }

  async removeCommand(commandId: string): Promise<void> {
    const database = await this.#open();
    const transaction = database.transaction(OUTBOX, "readwrite");
    const completed = transactionDone(transaction);
    const store = transaction.objectStore(OUTBOX);
    const key = store.index("commandId").getKey(commandId);
    key.onsuccess = () => {
      if (key.result !== undefined) {
        store.delete(key.result);
      }
    };
    await completed;
  }

  async persistEvent(event: TaskEvent): Promise<void> {
    const database = await this.#open();
    const transaction = database.transaction(EVENTS, "readwrite");
    const completed = transactionDone(transaction);
    const store = transaction.objectStore(EVENTS);
    const lookup = store.get(event.eventId) as IDBRequest<TaskEvent | undefined>;
    let changed: Error | undefined;
    lookup.onsuccess = () => {
      if (lookup.result === undefined) {
        store.add(event);
      } else if (JSON.stringify(lookup.result) !== JSON.stringify(event)) {
        changed = new Error(`task event ${event.eventId} changed during replay`);
        transaction.abort();
      }
    };
    try {
      await completed;
    } catch (failure) {
      if (changed !== undefined) {
        throw changed;
      }
      throw failure;
    }
  }

  async eventsForTask(taskId: string): Promise<readonly TaskEvent[]> {
    const database = await this.#open();
    const transaction = database.transaction(EVENTS, "readonly");
    const [events] = await Promise.all([
      request<TaskEvent[]>(
        transaction.objectStore(EVENTS).index("taskId").getAll(IDBKeyRange.only(taskId)),
      ),
      transactionDone(transaction),
    ]);
    return events.sort((left, right) => left.sequence - right.sequence);
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    const database = await this.#database;
    database.close();
    this.#closed = true;
  }

  async #open(): Promise<IDBDatabase> {
    if (this.#closed) {
      throw new Error("control-room state is closed");
    }
    return this.#database;
  }
}

function openDatabase(name: string): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const opening = indexedDB.open(name, DATABASE_VERSION);
    opening.onupgradeneeded = () => {
      const database = opening.result;
      database.createObjectStore(CURSORS, { keyPath: "taskId" });
      const outbox = database.createObjectStore(OUTBOX, {
        autoIncrement: true,
        keyPath: "ordinal",
      });
      outbox.createIndex("commandId", "commandId", { unique: true });
      const events = database.createObjectStore(EVENTS, { keyPath: "eventId" });
      events.createIndex("taskId", "taskId");
      events.createIndex("taskSequence", ["taskId", "sequence"], { unique: true });
    };
    opening.onsuccess = () => resolve(opening.result);
    opening.onerror = () => reject(opening.error ?? new Error("could not open browser state"));
    opening.onblocked = () => reject(new Error("browser state upgrade is blocked"));
  });
}

function request<T>(operation: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    operation.onsuccess = () => resolve(operation.result);
    operation.onerror = () => reject(operation.error ?? new Error("browser state request failed"));
  });
}

function transactionDone(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () =>
      reject(transaction.error ?? new Error("browser state transaction failed"));
    transaction.onabort = () =>
      reject(transaction.error ?? new Error("browser state transaction was aborted"));
  });
}
