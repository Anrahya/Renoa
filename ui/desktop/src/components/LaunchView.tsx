import { type FormEvent, useState } from 'react';
import type { AcpClient, ConnectionStatus } from '@acp-components/core';
import { loadLastSession } from '../acp/persistence';
import { createRenoaSession, loadRenoaSession } from '../acp/sessionController';
import { pickWorkspace } from '../acp/native';
import { ConnectionBadge } from './ConnectionBadge';

interface LaunchViewProps {
  client: AcpClient | null;
  connection: ConnectionStatus;
  ready: boolean;
  statusText: string;
}

type LaunchMode = 'new' | 'load';

export function LaunchView({ client, connection, ready, statusText }: LaunchViewProps) {
  const saved = loadLastSession();
  const [mode, setMode] = useState<LaunchMode>(saved ? 'load' : 'new');
  const [cwd, setCwd] = useState(saved?.cwd ?? '');
  const [sessionId, setSessionId] = useState(saved?.sessionId ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const connected = ready && connection === 'connected' && client !== null;

  const browse = async () => {
    setError(null);
    try {
      const selected = await pickWorkspace();
      if (selected) setCwd(selected);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The folder picker failed');
    }
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!client || !connected || busy) return;
    const workspace = cwd.trim();
    if (!workspace) {
      setError('Choose an absolute workspace path');
      return;
    }
    const requestedSession = sessionId.trim();
    if (mode === 'load' && !requestedSession) {
      setError('Enter a Renoa session ID');
      return;
    }

    setBusy(true);
    setError(null);
    try {
      if (mode === 'new') {
        await createRenoaSession(client, workspace);
      } else {
        await loadRenoaSession(client, requestedSession, workspace);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The session could not be opened');
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="launch">
      <section className="launch__brand" aria-labelledby="launch-title">
        <div className="wordmark" aria-label="Renoa">
          <span className="wordmark__mark">R</span>
          <span>Renoa</span>
        </div>
        <div>
          <h1 id="launch-title">Work with the repository in front of you.</h1>
          <p>
            Renoa runs locally through its ACP adapter. Choose a workspace, then
            start or resume one durable session.
          </p>
        </div>
        <ConnectionBadge status={connection} text={statusText} />
      </section>

      <section className="launch__panel" aria-label="Open a Renoa session">
        <div className="segmented" aria-label="Session action">
          <button
            type="button"
            aria-pressed={mode === 'new'}
            onClick={() => setMode('new')}
          >
            New session
          </button>
          <button
            type="button"
            aria-pressed={mode === 'load'}
            onClick={() => setMode('load')}
          >
            Load session
          </button>
        </div>

        <form className="launch-form" onSubmit={submit}>
          <div className="field">
            <label htmlFor="workspace">Workspace</label>
            <div className="field__row">
              <input
                id="workspace"
                value={cwd}
                onChange={(event) => setCwd(event.target.value)}
                placeholder="/absolute/path/to/repository"
                autoComplete="off"
                spellCheck={false}
              />
              <button className="button button--quiet" type="button" onClick={browse}>
                Browse
              </button>
            </div>
            <p className="field__hint">The ACP session is durably bound to this path.</p>
          </div>

          {mode === 'load' && (
            <div className="field">
              <label htmlFor="session-id">Session ID</label>
              <input
                id="session-id"
                value={sessionId}
                onChange={(event) => setSessionId(event.target.value)}
                placeholder="00000000-0000-0000-0000-000000000000"
                autoComplete="off"
                spellCheck={false}
              />
              <p className="field__hint">Use the ID shown when the session was created.</p>
            </div>
          )}

          {error && <p className="inline-error" role="alert">{error}</p>}

          <button className="button button--primary launch-form__submit" type="submit" disabled={!connected || busy}>
            {busy ? 'Opening session' : mode === 'new' ? 'Start session' : 'Resume session'}
          </button>
        </form>
      </section>
    </main>
  );
}
