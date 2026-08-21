import { useEffect, useRef } from 'react';
import { sessionStore } from '@acp-components/core';
import { useStore } from 'zustand/react';
import { MessageView } from './MessageView';

interface ConversationProps {
  sessionId: string;
}

export function Conversation({ sessionId }: ConversationProps) {
  const messages = useStore(
    sessionStore,
    (state) => state.sessions.get(sessionId)?.messages ?? [],
  );
  const streaming = useStore(
    sessionStore,
    (state) => state.sessions.get(sessionId)?.isStreaming ?? false,
  );
  const end = useRef<HTMLDivElement>(null);

  useEffect(() => {
    end.current?.scrollIntoView({ block: 'end' });
  }, [messages, streaming]);

  return (
    <section className="conversation" aria-label="Conversation" aria-live="polite">
      <div className="conversation__inner">
        {messages.length === 0 ? (
          <div className="conversation__empty">
            <h2>What should Renoa work on?</h2>
            <p>Ask a question, request a review, or describe a change in this workspace.</p>
          </div>
        ) : (
          messages.map((message) => (
            <MessageView key={message.id} message={message} streaming={streaming} />
          ))
        )}
        {streaming && (
          <div className="working" role="status">
            <span aria-hidden="true" />
            Renoa is working
          </div>
        )}
        <div ref={end} />
      </div>
    </section>
  );
}
