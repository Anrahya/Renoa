import { useMemo, useState, type FormEvent } from "react";
import {
  ArrowClockwise,
  ArrowRight,
  Books,
  CheckCircle,
  Clock,
  Code,
  Gear,
  HouseLine,
  LockSimple,
  PaperPlaneTilt,
  Play,
  Pulse,
  SignOut,
  HardDrives,
  Stack,
  TerminalWindow,
  WarningCircle,
} from "@phosphor-icons/react";

import { HistoryDrawer } from "./history-drawer";
import { projectTask, taskKind, type TaskState, type TaskView } from "./task-model";
import type { ControlRoomController } from "./use-control-room";

export function Workspace({ control, preview = false }: { readonly control: ControlRoomController; readonly preview?: boolean }) {
  const [historyOpen, setHistoryOpen] = useState(false);
  const [composerOpen, setComposerOpen] = useState(false);
  const [text, setText] = useState("");
  const taskViews = useMemo(
    () => control.tasks.map((task) => projectTask(task, control.events[task.taskId] ?? [])),
    [control.events, control.tasks],
  );
  const selected = taskViews.find((task) => task.taskId === control.selectedTaskId);
  const selectedEvents = selected === undefined ? [] : (control.events[selected.taskId] ?? []);

  async function submit(event: FormEvent): Promise<void> {
    event.preventDefault();
    const command = text.trim();
    if (command === "") {
      return;
    }
    try {
      await control.submit(command);
      setText("");
      setComposerOpen(false);
    } catch {
      // The controller exposes the exact failure in the persistent error banner.
    }
  }

  return (
    <div className="app-shell">
      <Header control={control} preview={preview} />
      {control.error !== null && (
        <div className="error-banner" role="alert">
          <WarningCircle size={18} weight="fill" aria-hidden="true" />
          <span>{control.error}</span>
          {control.connection === "disconnected" && (
            <button onClick={() => void control.reconnect()} disabled={control.busy}>
              Reconnect
            </button>
          )}
        </div>
      )}
      {control.pendingCount > 0 && (
        <div className="pending-banner">
          <ArrowClockwise size={18} aria-hidden="true" />
          <span>{control.pendingCount} durable command{control.pendingCount === 1 ? "" : "s"} awaiting acknowledgement</span>
          <button onClick={() => void control.retryPending()} disabled={control.busy || control.connection !== "connected"}>
            Retry
          </button>
        </div>
      )}
      <main className="workspace-grid">
        <aside className="task-rail" aria-label="Tasks">
          <div className="rail-heading">
            <div>
              <span className="eyebrow">Your work</span>
              <h1>Tasks</h1>
            </div>
            <button
              className="icon-button"
              title="Refresh tasks"
              onClick={() => void control.refresh()}
              disabled={control.busy || control.connection !== "connected"}
            >
              <ArrowClockwise size={19} aria-hidden="true" />
              <span className="sr-only">Refresh tasks</span>
            </button>
          </div>
          <div className="task-list">
            {taskViews.length === 0 ? (
              <div className="empty-tasks">
                <Stack size={26} aria-hidden="true" />
                <strong>No tasks assigned</strong>
                <span>The Host returned an empty task list.</span>
              </div>
            ) : (
              taskViews.map((task) => (
                <TaskPod
                  key={task.taskId}
                  task={task}
                  selected={task.taskId === control.selectedTaskId}
                  disabled={control.connection !== "connected"}
                  onSelect={() => void control.selectTask(task.taskId)}
                />
              ))
            )}
          </div>
        </aside>

        <section className="task-stage" aria-live="polite">
          {selected === undefined ? (
            <div className="no-selection">
              <img src="/assets/task-pod.png" alt="Neutral Renoa task pod" />
              <h2>Select a task</h2>
              <p>Open one of the Host tasks to load its durable history.</p>
            </div>
          ) : (
            <>
              <div className="stage-heading">
                <div>
                  <span className="eyebrow">{taskKind(selected.target)}</span>
                  <h2>{selected.title}</h2>
                  <p>{selected.target}</p>
                </div>
                <StateBadge state={selected.state} />
              </div>

              <div className={`task-console state-${selected.state}`}>
                <img src="/assets/task-console.png" alt="Renoa task console" />
                <div className="console-light" aria-hidden="true" />
                <div className="console-state" title={selected.detail}>
                  <StateIcon state={selected.state} size={20} />
                  <div>
                    <strong>{stateLabel(selected.state)}</strong>
                    <span>{selected.detail}</span>
                  </div>
                </div>
                {selected.state === "working" && <div className="working-track"><span /></div>}
              </div>

              <div className="task-facts" aria-label="Task metadata">
                <Fact icon={Pulse} label="Events" value={String(selected.eventCount)} />
                <Fact icon={Clock} label="Last event" value={formatTime(selected.lastRecordedAtMs)} />
                <Fact icon={TerminalWindow} label="Target" value={selected.target} wide />
              </div>

              <div className="stage-actions">
                <button className="secondary-button" onClick={() => setHistoryOpen(true)}>
                  <Pulse size={18} aria-hidden="true" />
                  View history
                </button>
                <button
                  className="primary-button"
                  onClick={() => setComposerOpen((open) => !open)}
                  disabled={control.connection !== "connected"}
                >
                  <Play size={18} weight="fill" aria-hidden="true" />
                  Continue task
                </button>
              </div>

              {composerOpen && (
                <form className="composer" onSubmit={(event) => void submit(event)}>
                  <label htmlFor="task-command">What should Renoa do next?</label>
                  <div className="composer-row">
                    <textarea
                      id="task-command"
                      value={text}
                      onChange={(event) => setText(event.target.value)}
                      placeholder="Continue with…"
                      rows={3}
                      autoFocus
                    />
                    <button className="send-button" type="submit" disabled={control.busy || text.trim() === ""} title="Send command">
                      <PaperPlaneTilt size={20} weight="fill" aria-hidden="true" />
                      <span className="sr-only">Send command</span>
                    </button>
                  </div>
                </form>
              )}
            </>
          )}
        </section>
      </main>
      <HistoryDrawer events={selectedEvents} open={historyOpen} onClose={() => setHistoryOpen(false)} />
    </div>
  );
}

function Header({ control, preview }: { readonly control: ControlRoomController; readonly preview: boolean }) {
  return (
    <header className="topbar">
      <a className="brand" href="/" aria-label="Renoa home">renoa<span /></a>
      <nav aria-label="Primary navigation">
        <button className="nav-item active"><Stack size={20} aria-hidden="true" />Tasks</button>
        <button className="nav-item" disabled title="Available when the Host exposes the agent directory"><HouseLine size={20} aria-hidden="true" />Office</button>
        <button className="nav-item" disabled title="Available when the Host exposes its capability catalog"><Books size={20} aria-hidden="true" />Library</button>
        <button className="nav-item" disabled title="Host settings are not exposed over RCP yet"><Gear size={20} aria-hidden="true" />Settings</button>
      </nav>
      <div className="header-tools">
        {preview && <span className="preview-badge">Design preview</span>}
        <div className={`connection-pill ${control.connection}`} title="RCP connection state">
          <span />
          <div><strong>{window.location.host || "Renoa Host"}</strong><small>{connectionLabel(control.connection)}</small></div>
        </div>
        <button className="identity-button" onClick={() => void control.leave()} title="Lock Control Room">
          <div className="avatar"><LockSimple size={17} aria-hidden="true" /></div>
          <span>{shortId(control.savedPrincipalId)}</span>
          <SignOut size={17} aria-hidden="true" />
        </button>
      </div>
    </header>
  );
}

function TaskPod({ task, selected, disabled, onSelect }: { readonly task: TaskView; readonly selected: boolean; readonly disabled: boolean; readonly onSelect: () => void }) {
  const PodIcon = task.target.startsWith("telegram:")
    ? PaperPlaneTilt
    : task.target.startsWith("service:")
      ? HardDrives
      : Code;
  return (
    <button className={`task-pod ${selected ? "selected" : ""}`} onClick={onSelect} aria-pressed={selected} disabled={disabled}>
      <div className="pod-visual"><PodIcon size={22} weight="duotone" aria-hidden="true" /></div>
      <div className="pod-copy"><strong>{task.title}</strong><span>{task.target}</span></div>
      <div className={`pod-state ${task.state}`} title={stateLabel(task.state)}><StateIcon state={task.state} size={15} /></div>
      <ArrowRight className="pod-arrow" size={17} aria-hidden="true" />
    </button>
  );
}

function StateBadge({ state }: { readonly state: TaskState }) {
  return <div className={`state-badge ${state}`}><StateIcon state={state} size={16} /><span>{stateLabel(state)}</span></div>;
}

function StateIcon({ state, size }: { readonly state: TaskState; readonly size: number }) {
  if (state === "ready") return <CheckCircle size={size} weight="fill" aria-hidden="true" />;
  if (state === "working" || state === "queued") return <Pulse size={size} weight="bold" aria-hidden="true" />;
  return <WarningCircle size={size} weight="fill" aria-hidden="true" />;
}

function Fact({ icon: Icon, label, value, wide = false }: { readonly icon: typeof Pulse; readonly label: string; readonly value: string; readonly wide?: boolean }) {
  return <div className={`fact ${wide ? "wide" : ""}`} title={`${label}: ${value}`}><Icon size={19} aria-hidden="true" /><div><span>{label}</span><strong>{value}</strong></div></div>;
}

function stateLabel(state: TaskState): string {
  if (state === "ready") return "Ready";
  if (state === "queued") return "Queued";
  if (state === "working") return "Working";
  if (state === "failed") return "Needs attention";
  return "Cancelled";
}

function connectionLabel(state: ControlRoomController["connection"]): string {
  if (state === "connected") return "Connected";
  if (state === "connecting") return "Connecting";
  if (state === "disconnected") return "Disconnected";
  return "Locked";
}

function formatTime(value: number | null): string {
  if (value === null) return "No timestamp";
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(value);
}

function shortId(value: string): string {
  return value === "" ? "Passkey" : `${value.slice(0, 8)}…`;
}
