import { type FormEvent, type KeyboardEvent, useState } from 'react';
import { cancelPrompt, sendPrompt, sessionStore, type AcpClient } from '@acp-components/core';
import { useStore } from 'zustand/react';

interface ComposerProps {
  client: AcpClient | null;
  sessionId: string;
}

export function Composer({ client, sessionId }: ComposerProps) {
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const streaming = useStore(
    sessionStore,
    (state) => state.sessions.get(sessionId)?.isStreaming ?? false,
  );

  const submit = async () => {
    const text = draft.trim();
    if (!client || !text || streaming) return;
    setDraft('');
    setError(null);
    try {
      await sendPrompt(client, sessionId, [{ type: 'text', text }]);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The prompt failed');
    }
  };

  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    void submit();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      void submit();
    }
  };

  const cancel = async () => {
    if (!client || !streaming) return;
    setCancelling(true);
    setError(null);
    try {
      await cancelPrompt(client, sessionId);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Cancellation failed');
    } finally {
      setCancelling(false);
    }
  };

  return (
    <div className="composer-wrap">
      <form className="composer" aria-label="Prompt Renoa" onSubmit={onSubmit}>
        <label className="sr-only" htmlFor="prompt">Prompt</label>
        <textarea
          id="prompt"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Ask Renoa to inspect, explain, or change this workspace"
          rows={3}
          disabled={!client}
        />
        <div className="composer__actions">
          <span>Enter to send, Shift+Enter for a new line</span>
          {streaming ? (
            <button className="button button--stop" type="button" onClick={() => void cancel()} disabled={cancelling}>
              {cancelling ? 'Stopping' : 'Stop'}
            </button>
          ) : (
            <button className="button button--primary" type="submit" disabled={!client || draft.trim().length === 0}>
              Send
            </button>
          )}
        </div>
      </form>
      {error && <p className="inline-error composer-wrap__error" role="alert">{error}</p>}
    </div>
  );
}
