import {
  ArrowCounterClockwise,
  ChatCircleText,
  CheckCircle,
  CircleNotch,
  ClockCounterClockwise,
  Play,
  TerminalWindow,
  WarningCircle,
  X,
} from "@phosphor-icons/react";
import { useEffect } from "react";
import type { TaskEvent } from "@renoa/rcp-client/browser";

interface HistoryDrawerProps {
  readonly events: readonly TaskEvent[];
  readonly open: boolean;
  readonly onClose: () => void;
}

export function HistoryDrawer({ events, open, onClose }: HistoryDrawerProps) {
  useEffect(() => {
    if (!open) return undefined;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [onClose, open]);

  if (!open) {
    return null;
  }
  return (
    <div className="drawer-backdrop" role="presentation" onMouseDown={onClose}>
      <aside
        className="history-drawer"
        aria-label="Durable task history"
        aria-modal="true"
        role="dialog"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="drawer-header">
          <div>
            <span className="eyebrow">Authoritative replay</span>
            <h2>Task history</h2>
          </div>
          <button className="icon-button" onClick={onClose} title="Close history" autoFocus>
            <X size={20} weight="bold" aria-hidden="true" />
            <span className="sr-only">Close history</span>
          </button>
        </header>
        <div className="history-list">
          {events.length === 0 ? (
            <div className="empty-history">
              <ClockCounterClockwise size={26} aria-hidden="true" />
              <p>No durable events yet.</p>
            </div>
          ) : (
            events.map((event) => <HistoryItem key={event.eventId} event={event} />)
          )}
        </div>
      </aside>
    </div>
  );
}

function HistoryItem({ event }: { readonly event: TaskEvent }) {
  const view = eventView(event);
  const Icon = view.icon;
  return (
    <article className={`history-item ${view.tone}`}>
      <div className="history-icon">
        <Icon size={17} weight="bold" aria-hidden="true" />
      </div>
      <div className="history-copy">
        <div className="history-title-row">
          <strong>{view.title}</strong>
          <span>#{event.sequence}</span>
        </div>
        {view.text !== null && <p>{view.text}</p>}
        {view.detail !== null && (
          <details>
            <summary>Details</summary>
            <pre>{view.detail}</pre>
          </details>
        )}
        <time>{view.time}</time>
      </div>
    </article>
  );
}

function eventView(event: TaskEvent) {
  if (event.kind.type === "command_submitted") {
    return {
      icon: Play,
      tone: "neutral",
      title: "Command admitted",
      text: event.kind.command.input.text,
      detail: null,
      time: "Durable command",
    };
  }
  const execution = event.kind.event;
  const time = new Date(execution.recordedAtMs).toLocaleString();
  switch (execution.kind.type) {
    case "execution_started":
      return { icon: CircleNotch, tone: "active", title: "Execution started", text: null, detail: null, time };
    case "turn_started":
      return { icon: ArrowCounterClockwise, tone: "active", title: "Turn started", text: null, detail: null, time };
    case "assistant_message":
      return { icon: ChatCircleText, tone: "neutral", title: "Assistant message", text: execution.kind.text, detail: null, time };
    case "tool_started":
      return {
        icon: TerminalWindow,
        tone: "active",
        title: `Started ${execution.kind.name}`,
        text: null,
        detail: JSON.stringify(execution.kind.arguments, null, 2),
        time,
      };
    case "tool_finished":
      return {
        icon: execution.kind.is_error ? WarningCircle : CheckCircle,
        tone: execution.kind.is_error ? "danger" : "success",
        title: execution.kind.is_error ? "Tool failed" : "Tool finished",
        text: null,
        detail: execution.kind.output,
        time,
      };
    case "execution_terminated": {
      const terminal = execution.kind.terminal;
      if (terminal.status === "completed") {
        return { icon: CheckCircle, tone: "success", title: "Execution completed", text: null, detail: null, time };
      }
      const text = terminal.status === "failed" ? terminal.error : terminal.reason;
      return { icon: WarningCircle, tone: "danger", title: `Execution ${terminal.status}`, text, detail: null, time };
    }
  }
}
