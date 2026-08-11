import { randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";

import type {
  ExecuteCommand,
  ExecutionEvent,
  ExecutionEventKind,
  ExecutionTerminal,
  QueuedExecution,
} from "./protocol.js";

const SCHEMA_VERSION = 2;

export interface Admission {
  readonly executionId: string;
  readonly admitted: boolean;
}

export interface PendingPublication {
  readonly commandId: string;
  readonly taskId: string;
  readonly admissionAcked: boolean;
  readonly events: readonly ExecutionEvent[];
}

export class NodeState {
  readonly #database: DatabaseSync;
  readonly #onCommit: () => void;
  #closed = false;

  constructor(path: string, onCommit: () => void = () => {}) {
    this.#database = new DatabaseSync(path);
    this.#onCommit = onCommit;
    const schema = this.#database.prepare("PRAGMA user_version").get() as {
      readonly user_version: number;
    };
    if (schema.user_version > SCHEMA_VERSION) {
      this.#database.close();
      throw new Error(
        `Pi node database schema ${schema.user_version} is newer than supported version ${SCHEMA_VERSION}`,
      );
    }
    if (schema.user_version === 1) {
      try {
        migrateHarnessFields(this.#database);
      } catch (error) {
        this.#database.close();
        throw error;
      }
    }
    this.#database.exec(`
      PRAGMA foreign_keys = ON;
      PRAGMA journal_mode = WAL;
      PRAGMA synchronous = FULL;
      CREATE TABLE IF NOT EXISTS executions (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        command_id TEXT NOT NULL UNIQUE,
        execution_id TEXT NOT NULL UNIQUE,
        task_id TEXT NOT NULL,
        command_json TEXT NOT NULL,
        status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'terminal')),
        admission_acked INTEGER NOT NULL DEFAULT 0 CHECK (admission_acked IN (0, 1)),
        published_through INTEGER
          CHECK (published_through IS NULL OR published_through BETWEEN 0 AND 9007199254740991)
      ) STRICT;
      CREATE TABLE IF NOT EXISTS execution_events (
        command_id TEXT NOT NULL REFERENCES executions(command_id) ON DELETE CASCADE,
        sequence INTEGER NOT NULL CHECK (sequence BETWEEN 0 AND 9007199254740991),
        event_id TEXT NOT NULL UNIQUE,
        execution_id TEXT NOT NULL,
        recorded_at_ms INTEGER NOT NULL
          CHECK (recorded_at_ms BETWEEN -9007199254740991 AND 9007199254740991),
        kind_json TEXT NOT NULL,
        PRIMARY KEY (command_id, sequence)
      ) STRICT;
      CREATE TABLE IF NOT EXISTS task_contexts (
        task_id TEXT PRIMARY KEY,
        messages_json TEXT NOT NULL
      ) STRICT;
      PRAGMA user_version = 2;
    `);
  }

  admit(command: ExecuteCommand): Admission {
    const commandJson = encodeCommand(command);
    const existing = this.#database
      .prepare("SELECT execution_id, command_json FROM executions WHERE command_id = ?")
      .get(command.commandId) as
      | { readonly execution_id: string; readonly command_json: string }
      | undefined;
    if (existing !== undefined) {
      if (existing.command_json !== commandJson) {
        throw new Error(`command ${command.commandId} does not match its durable admission`);
      }
      return { executionId: existing.execution_id, admitted: false };
    }

    const executionId = randomUUID();
    this.#database
      .prepare(`
        INSERT INTO executions (
          command_id, execution_id, task_id, command_json, status
        ) VALUES (?, ?, ?, ?, 'queued')
      `)
      .run(command.commandId, executionId, command.taskId, commandJson);
    this.#onCommit();
    return { executionId, admitted: true };
  }

  nextQueued(): QueuedExecution | null {
    const row = this.#database
      .prepare(`
        SELECT execution_id, command_json
        FROM executions
        WHERE status = 'queued'
        ORDER BY ordinal
        LIMIT 1
      `)
      .get() as
      | { readonly execution_id: string; readonly command_json: string }
      | undefined;
    if (row === undefined) {
      return null;
    }
    return {
      ...decodeCommand(row.command_json),
      executionId: row.execution_id,
    };
  }

  claimNext(): QueuedExecution | null {
    const queued = this.nextQueued();
    if (queued === null) {
      return null;
    }
    return this.#transaction(() => {
      const changed = this.#database
        .prepare("UPDATE executions SET status = 'running' WHERE command_id = ? AND status = 'queued'")
        .run(queued.commandId).changes;
      if (changed !== 1) {
        throw new Error(`command ${queued.commandId} could not enter the running state`);
      }
      this.#appendEvent(queued.commandId, queued.executionId, {
        type: "execution_started",
      });
      return queued;
    });
  }

  appendEvent(commandId: string, kind: ExecutionEventKind): ExecutionEvent {
    return this.#transaction(() => {
      const execution = this.#runningExecution(commandId);
      return this.#appendEvent(commandId, execution.execution_id, kind);
    });
  }

  pendingPublications(): readonly PendingPublication[] {
    const rows = this.#database
      .prepare(`
        SELECT command_id, task_id, admission_acked, published_through
        FROM executions
        WHERE admission_acked = 0 OR EXISTS (
          SELECT 1 FROM execution_events
          WHERE execution_events.command_id = executions.command_id
            AND (executions.published_through IS NULL
              OR execution_events.sequence > executions.published_through)
        )
        ORDER BY ordinal
      `)
      .all() as Array<{
      readonly command_id: string;
      readonly task_id: string;
      readonly admission_acked: number;
      readonly published_through: number | null;
    }>;
    return rows.map((row) => ({
      commandId: row.command_id,
      taskId: row.task_id,
      admissionAcked: row.admission_acked === 1,
      events: this.#eventsAfter(row.command_id, row.published_through),
    }));
  }

  acknowledgeAdmission(commandId: string): void {
    const changed = this.#database
      .prepare("UPDATE executions SET admission_acked = 1 WHERE command_id = ?")
      .run(commandId).changes;
    if (changed !== 1) {
      throw new Error(`cannot acknowledge unknown command ${commandId}`);
    }
    this.#onCommit();
  }

  advancePublication(commandId: string, throughSequence: number): void {
    safeSequence(throughSequence);
    const row = this.#database
      .prepare(`
        SELECT published_through,
          EXISTS(
            SELECT 1 FROM execution_events
            WHERE execution_events.command_id = executions.command_id AND sequence = ?
          ) AS sequence_exists
        FROM executions WHERE command_id = ?
      `)
      .get(throughSequence, commandId) as
      | { readonly published_through: number | null; readonly sequence_exists: number }
      | undefined;
    if (row === undefined || row.sequence_exists !== 1) {
      throw new Error(`cannot advance command ${commandId} to unknown sequence ${throughSequence}`);
    }
    if (row.published_through !== null && throughSequence < row.published_through) {
      throw new Error(`publication cursor for command ${commandId} cannot move backwards`);
    }
    this.#database
      .prepare("UPDATE executions SET published_through = ? WHERE command_id = ?")
      .run(throughSequence, commandId);
    this.#onCommit();
  }

  recoverInterrupted(): void {
    this.#transaction(() => {
      const rows = this.#database
        .prepare("SELECT command_id, execution_id FROM executions WHERE status = 'running'")
        .all() as Array<{ readonly command_id: string; readonly execution_id: string }>;
      for (const row of rows) {
        this.#appendEvent(row.command_id, row.execution_id, {
          type: "execution_terminated",
          terminal: {
            status: "failed",
            error: "execution interrupted by node restart",
          },
        });
        this.#database
          .prepare("UPDATE executions SET status = 'terminal' WHERE command_id = ?")
          .run(row.command_id);
      }
    });
  }

  loadMessages<T>(taskId: string): T[] {
    const row = this.#database
      .prepare("SELECT messages_json FROM task_contexts WHERE task_id = ?")
      .get(taskId) as { readonly messages_json: string } | undefined;
    return row === undefined ? [] : (JSON.parse(row.messages_json) as T[]);
  }

  finish(
    commandId: string,
    terminal: ExecutionTerminal,
    messages: readonly unknown[],
  ): ExecutionEvent {
    return this.#transaction(() => {
      const execution = this.#runningExecution(commandId);
      const event = this.#appendEvent(commandId, execution.execution_id, {
        type: "execution_terminated",
        terminal,
      });
      const changed = this.#database
        .prepare(
          "UPDATE executions SET status = 'terminal' WHERE command_id = ? AND status = 'running'",
        )
        .run(commandId).changes;
      if (changed !== 1) {
        throw new Error(`command ${commandId} could not enter the terminal state`);
      }
      this.#database
        .prepare(`
          INSERT INTO task_contexts (task_id, messages_json)
          SELECT task_id, ? FROM executions WHERE command_id = ?
          ON CONFLICT (task_id) DO UPDATE SET messages_json = excluded.messages_json
        `)
        .run(JSON.stringify(messages), commandId);
      return event;
    });
  }

  #runningExecution(commandId: string): { readonly execution_id: string } {
    const row = this.#database
      .prepare("SELECT execution_id FROM executions WHERE command_id = ? AND status = 'running'")
      .get(commandId) as { readonly execution_id: string } | undefined;
    if (row === undefined) {
      throw new Error(`command ${commandId} is not running`);
    }
    return row;
  }

  #appendEvent(
    commandId: string,
    executionId: string,
    kind: ExecutionEventKind,
  ): ExecutionEvent {
    const row = this.#database
      .prepare("SELECT COALESCE(MAX(sequence) + 1, 0) AS sequence FROM execution_events WHERE command_id = ?")
      .get(commandId) as { readonly sequence: number };
    const event: ExecutionEvent = {
      eventId: randomUUID(),
      executionId,
      sequence: safeSequence(row.sequence),
      recordedAtMs: Date.now(),
      kind,
    };
    this.#database
      .prepare(`
        INSERT INTO execution_events (
          command_id, sequence, event_id, execution_id, recorded_at_ms, kind_json
        ) VALUES (?, ?, ?, ?, ?, ?)
      `)
      .run(
        commandId,
        event.sequence,
        event.eventId,
        event.executionId,
        event.recordedAtMs,
        JSON.stringify(event.kind),
      );
    return event;
  }

  #eventsAfter(commandId: string, sequence: number | null): readonly ExecutionEvent[] {
    const rows = this.#database
      .prepare(`
        SELECT event_id, execution_id, sequence, recorded_at_ms, kind_json
        FROM execution_events
        WHERE command_id = ? AND (? IS NULL OR sequence > ?)
        ORDER BY sequence
      `)
      .all(commandId, sequence, sequence) as Array<{
      readonly event_id: string;
      readonly execution_id: string;
      readonly sequence: number;
      readonly recorded_at_ms: number;
      readonly kind_json: string;
    }>;
    return rows.map((row) => ({
      eventId: row.event_id,
      executionId: row.execution_id,
      sequence: safeSequence(row.sequence),
      recordedAtMs: row.recorded_at_ms,
      kind: JSON.parse(row.kind_json) as ExecutionEventKind,
    }));
  }

  #transaction<T>(operation: () => T): T {
    this.#database.exec("BEGIN IMMEDIATE");
    let result: T;
    try {
      result = operation();
      this.#database.exec("COMMIT");
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
    this.#onCommit();
    return result;
  }

  close(): void {
    if (!this.#closed) {
      this.#database.close();
      this.#closed = true;
    }
  }
}

function safeSequence(value: number): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`invalid execution sequence ${value}`);
  }
  return value;
}

function encodeCommand(command: ExecuteCommand): string {
  return JSON.stringify({
    taskId: command.taskId,
    commandId: command.commandId,
    principalId: command.principalId,
    surface: command.surface,
    target: command.target,
    text: command.text,
  });
}

function decodeCommand(json: string): ExecuteCommand {
  return JSON.parse(json) as ExecuteCommand;
}

function migrateHarnessFields(database: DatabaseSync): void {
  database.exec("BEGIN IMMEDIATE");
  try {
    const rows = database.prepare("SELECT command_id, command_json FROM executions").all() as Array<{
      readonly command_id: string;
      readonly command_json: string;
    }>;
    const update = database.prepare(
      "UPDATE executions SET command_json = ? WHERE command_id = ?",
    );
    for (const row of rows) {
      update.run(encodeCommand(decodeCommand(row.command_json)), row.command_id);
    }
    database.exec("PRAGMA user_version = 2; COMMIT");
  } catch (error) {
    database.exec("ROLLBACK");
    throw error;
  }
}
