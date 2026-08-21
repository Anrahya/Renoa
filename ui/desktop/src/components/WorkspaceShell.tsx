import { acpStore, sessionStore, type AcpClient, type ConnectionStatus } from '@acp-components/core';
import { useStore } from 'zustand/react';
import { Composer } from './Composer';
import { ConfigControls } from './ConfigControls';
import { ConnectionBadge } from './ConnectionBadge';
import { Conversation } from './Conversation';

interface WorkspaceShellProps {
  client: AcpClient | null;
  connection: ConnectionStatus;
  sessionId: string;
  statusText: string;
}

function workspaceName(cwd: string): string {
  const normalized = cwd.replace(/[/\\]+$/, '');
  const segments = normalized.split(/[/\\]/);
  return segments.at(-1) || cwd;
}

export function WorkspaceShell({ client, connection, sessionId, statusText }: WorkspaceShellProps) {
  const session = useStore(acpStore, (state) => {
    for (const workspace of state.workspaces.values()) {
      const found = workspace.sessions.get(sessionId);
      if (found) return found;
    }
    return null;
  });
  const streaming = useStore(
    sessionStore,
    (state) => state.sessions.get(sessionId)?.isStreaming ?? false,
  );
  const cwd = session?.cwd ?? '';

  return (
    <main className="workspace-shell">
      <aside className="sidebar">
        <div className="wordmark wordmark--small" aria-label="Renoa">
          <span className="wordmark__mark">R</span>
          <span>Renoa</span>
        </div>
        <div className="sidebar__context">
          <p className="sidebar__label">Workspace</p>
          <h1 title={cwd}>{workspaceName(cwd)}</h1>
          <p className="sidebar__path" title={cwd}>{cwd}</p>
        </div>
        <div className="sidebar__session">
          <p className="sidebar__label">Session</p>
          <code title={sessionId}>{sessionId}</code>
        </div>
        <div className="sidebar__status">
          <ConnectionBadge status={connection} text={statusText} />
        </div>
      </aside>

      <section className="workbench">
        <header className="workbench__header">
          <div>
            <p className="workbench__title">Current task</p>
            <p className="workbench__subtitle">ACP session</p>
          </div>
          <ConfigControls client={client} disabled={streaming} sessionId={sessionId} />
        </header>
        <Conversation sessionId={sessionId} />
        <Composer client={client} sessionId={sessionId} />
      </section>
    </main>
  );
}
