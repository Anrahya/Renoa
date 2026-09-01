import { useCallback, useEffect, useRef, useState } from "react";
import {
  RcpSurfaceClient,
  type TaskEvent,
  type TaskSummary,
} from "@renoa/rcp-client/browser";

import { authenticatePasskey, rcpEndpoint, registerPasskey } from "./passkeys";
import { ControlRoomStore } from "./rcp-store";

const PRINCIPAL_KEY = "renoa.control-room.principal-id";

type ConnectionState = "locked" | "connecting" | "connected" | "disconnected";
type EventJournal = Readonly<Record<string, readonly TaskEvent[]>>;

interface Runtime {
  readonly client: RcpSurfaceClient;
  readonly store: ControlRoomStore;
  readonly attached: Set<string>;
}

export interface ControlRoomController {
  readonly connection: ConnectionState;
  readonly error: string | null;
  readonly tasks: readonly TaskSummary[];
  readonly events: EventJournal;
  readonly selectedTaskId: string | null;
  readonly pendingCount: number;
  readonly busy: boolean;
  readonly savedPrincipalId: string;
  readonly unlock: (principalId: string) => Promise<void>;
  readonly register: (principalId: string, bootstrapToken: string) => Promise<void>;
  readonly reconnect: () => Promise<void>;
  readonly leave: () => Promise<void>;
  readonly refresh: () => Promise<void>;
  readonly selectTask: (taskId: string) => Promise<void>;
  readonly submit: (text: string) => Promise<void>;
  readonly retryPending: () => Promise<void>;
}

export function useControlRoom(): ControlRoomController {
  const runtime = useRef<Runtime | undefined>(undefined);
  const [connection, setConnection] = useState<ConnectionState>("locked");
  const [error, setError] = useState<string | null>(null);
  const [tasks, setTasks] = useState<readonly TaskSummary[]>([]);
  const [events, setEvents] = useState<EventJournal>({});
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [pendingCount, setPendingCount] = useState(0);
  const [busy, setBusy] = useState(false);
  const [savedPrincipalId, setSavedPrincipalId] = useState(readPrincipal);

  useEffect(() => {
    return () => {
      const active = runtime.current;
      runtime.current = undefined;
      if (active !== undefined) {
        void active.client.close();
      }
    };
  }, []);

  const applyEvent = useCallback(async (store: ControlRoomStore, event: TaskEvent) => {
    await store.persistEvent(event);
    setEvents((current) => {
      const journal = current[event.taskId] ?? [];
      if (journal.some((record) => record.eventId === event.eventId)) {
        return current;
      }
      return {
        ...current,
        [event.taskId]: [...journal, event].sort(
          (left, right) => left.sequence - right.sequence,
        ),
      };
    });
  }, []);

  const attachTask = useCallback(
    async (active: Runtime, taskId: string) => {
      if (active.attached.has(taskId)) {
        return;
      }
      await active.client.attach(taskId, (event) => applyEvent(active.store, event));
      active.attached.add(taskId);
    },
    [applyEvent],
  );

  const loadTasks = useCallback(
    async (active: Runtime) => {
      const listed = await active.client.listTasks();
      const journals = await Promise.all(
        listed.map(async (task) => [task.taskId, await active.store.eventsForTask(task.taskId)] as const),
      );
      setTasks(listed);
      setEvents(Object.fromEntries(journals));
      const selected = listed.some((task) => task.taskId === selectedTaskId)
        ? selectedTaskId
        : (listed[0]?.taskId ?? null);
      setSelectedTaskId(selected);
      if (selected !== null) {
        await attachTask(active, selected);
      }
    },
    [attachTask, selectedTaskId],
  );

  const monitor = useCallback((active: Runtime) => {
    void active.client.waitForDisconnect().then((reason) => {
      if (runtime.current === active) {
        setConnection("disconnected");
        setError(reason.message);
      }
    });
  }, []);

  const establish = useCallback(
    async (principalId: string, initialTicket?: string) => {
      setBusy(true);
      setConnection("connecting");
      setError(null);
      const store = new ControlRoomStore();
      let ticket = initialTicket;
      const client = new RcpSurfaceClient({
        endpoint: rcpEndpoint(),
        authentication: {
          type: "ticket",
          getTicket: async () => {
            if (ticket !== undefined) {
              const current = ticket;
              ticket = undefined;
              return current;
            }
            return (await authenticatePasskey(principalId)).connectionTicket;
          },
        },
        state: store,
      });
      const active: Runtime = { client, store, attached: new Set() };
      try {
        await client.connect();
        await loadTasks(active);
        setPendingCount((await store.pendingCommands()).length);
        rememberPrincipal(principalId);
        setSavedPrincipalId(principalId);
        runtime.current = active;
        setConnection("connected");
        monitor(active);
      } catch (failure) {
        if (runtime.current === active) {
          runtime.current = undefined;
        }
        await client.close();
        setConnection("locked");
        setError(message(failure));
      } finally {
        setBusy(false);
      }
    },
    [loadTasks, monitor],
  );

  const unlock = useCallback(
    async (principalId: string) => establish(principalId),
    [establish],
  );

  const register = useCallback(
    async (principalId: string, bootstrapToken: string) => {
      setBusy(true);
      setError(null);
      try {
        const grant = await registerPasskey(bootstrapToken);
        await establish(principalId, grant.connectionTicket);
      } catch (failure) {
        setConnection("locked");
        setError(message(failure));
      } finally {
        setBusy(false);
      }
    },
    [establish],
  );

  const reconnect = useCallback(async () => {
    const active = runtime.current;
    if (active === undefined) {
      throw new Error("Control Room has no connection to resume");
    }
    setBusy(true);
    setConnection("connecting");
    setError(null);
    try {
      await active.client.connect();
      await loadTasks(active);
      setPendingCount((await active.store.pendingCommands()).length);
      setConnection("connected");
      monitor(active);
    } catch (failure) {
      setConnection("disconnected");
      setError(message(failure));
    } finally {
      setBusy(false);
    }
  }, [loadTasks, monitor]);

  const leave = useCallback(async () => {
    const active = runtime.current;
    runtime.current = undefined;
    setConnection("locked");
    setTasks([]);
    setEvents({});
    setSelectedTaskId(null);
    setPendingCount(0);
    setError(null);
    if (active !== undefined) {
      await active.client.close();
    }
  }, []);

  const refresh = useCallback(async () => {
    const active = requireRuntime(runtime.current);
    setBusy(true);
    setError(null);
    try {
      await loadTasks(active);
    } catch (failure) {
      setError(message(failure));
    } finally {
      setBusy(false);
    }
  }, [loadTasks]);

  const selectTask = useCallback(
    async (taskId: string) => {
      const active = requireRuntime(runtime.current);
      setSelectedTaskId(taskId);
      setError(null);
      try {
        await attachTask(active, taskId);
      } catch (failure) {
        setError(message(failure));
      }
    },
    [attachTask],
  );

  const submit = useCallback(async (text: string) => {
    const active = requireRuntime(runtime.current);
    if (selectedTaskId === null) {
      throw new Error("Select a task before continuing it");
    }
    setBusy(true);
    setError(null);
    try {
      const submission = await active.client.submitText(selectedTaskId, text);
      setPendingCount((await active.store.pendingCommands()).length);
      await submission.accepted;
      setPendingCount((await active.store.pendingCommands()).length);
    } catch (failure) {
      setPendingCount((await active.store.pendingCommands()).length);
      setError(message(failure));
      throw failure;
    } finally {
      setBusy(false);
    }
  }, [selectedTaskId]);

  const retryPending = useCallback(async () => {
    const active = requireRuntime(runtime.current);
    setBusy(true);
    setError(null);
    try {
      await active.client.retryPendingCommands();
      setPendingCount((await active.store.pendingCommands()).length);
    } catch (failure) {
      setPendingCount((await active.store.pendingCommands()).length);
      setError(message(failure));
    } finally {
      setBusy(false);
    }
  }, []);

  return {
    connection,
    error,
    tasks,
    events,
    selectedTaskId,
    pendingCount,
    busy,
    savedPrincipalId,
    unlock,
    register,
    reconnect,
    leave,
    refresh,
    selectTask,
    submit,
    retryPending,
  };
}

function requireRuntime(runtime: Runtime | undefined): Runtime {
  if (runtime === undefined) {
    throw new Error("Control Room is not connected");
  }
  return runtime;
}

function message(failure: unknown): string {
  return failure instanceof Error ? failure.message : String(failure);
}

function readPrincipal(): string {
  try {
    return localStorage.getItem(PRINCIPAL_KEY) ?? "";
  } catch {
    return "";
  }
}

function rememberPrincipal(principalId: string): void {
  try {
    localStorage.setItem(PRINCIPAL_KEY, principalId);
  } catch {
    // The principal ID is a convenience only; passkey authentication remains usable.
  }
}
