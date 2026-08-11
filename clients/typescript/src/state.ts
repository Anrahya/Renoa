import { DatabaseSync } from "node:sqlite";

export interface PendingTextCommand {
  readonly commandId: string;
  readonly taskId: string;
  readonly text: string;
}

export class SurfaceState {
  readonly #database: DatabaseSync;
  #closed = false;

  constructor(path: string) {
    this.#database = new DatabaseSync(path);
    this.#database.exec(`
      PRAGMA journal_mode = WAL;
      PRAGMA synchronous = FULL;
      CREATE TABLE IF NOT EXISTS task_cursors (
        task_id TEXT PRIMARY KEY,
        sequence INTEGER NOT NULL
          CHECK (sequence BETWEEN 0 AND 9007199254740991)
      ) STRICT;
      CREATE TABLE IF NOT EXISTS command_outbox (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        command_id TEXT NOT NULL UNIQUE,
        task_id TEXT NOT NULL,
        text TEXT NOT NULL
      ) STRICT;
    `);
  }

  cursor(taskId: string): number | null {
    const row = this.#database
      .prepare("SELECT sequence FROM task_cursors WHERE task_id = ?")
      .get(taskId) as { readonly sequence: number } | undefined;
    if (row === undefined) {
      return null;
    }
    if (!Number.isSafeInteger(row.sequence) || row.sequence < 0) {
      throw new Error(`stored cursor for task ${taskId} is invalid`);
    }
    return row.sequence;
  }

  advanceCursor(taskId: string, sequence: number): void {
    this.#database
      .prepare(`
        INSERT INTO task_cursors (task_id, sequence) VALUES (?, ?)
        ON CONFLICT (task_id) DO UPDATE SET sequence = excluded.sequence
        WHERE task_cursors.sequence < excluded.sequence
      `)
      .run(taskId, sequence);
  }

  enqueueCommand(command: PendingTextCommand): void {
    this.#database
      .prepare("INSERT INTO command_outbox (command_id, task_id, text) VALUES (?, ?, ?)")
      .run(command.commandId, command.taskId, command.text);
  }

  pendingCommands(): readonly PendingTextCommand[] {
    const rows = this.#database
      .prepare(`
        SELECT command_id, task_id, text
        FROM command_outbox
        ORDER BY ordinal
      `)
      .all() as Array<{
      readonly command_id: string;
      readonly task_id: string;
      readonly text: string;
    }>;
    return rows.map((row) => ({
      commandId: row.command_id,
      taskId: row.task_id,
      text: row.text,
    }));
  }

  removeCommand(commandId: string): void {
    this.#database.prepare("DELETE FROM command_outbox WHERE command_id = ?").run(commandId);
  }

  close(): void {
    if (!this.#closed) {
      this.#database.close();
      this.#closed = true;
    }
  }
}
