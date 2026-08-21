import type { ContentBlock, Message, ToolCallState } from '@acp-components/core';

interface MessageViewProps {
  message: Message;
  streaming: boolean;
}

function contentLabel(block: ContentBlock): string {
  if (block.type === 'text') return block.text;
  if (block.type === 'image') return `[Image: ${block.mimeType}]`;
  if (block.type === 'resource_link') return block.name || block.uri;
  if (block.type === 'audio') return `[Audio: ${block.mimeType}]`;
  return '[Embedded resource]';
}

function statusLabel(status: ToolCallState['status']): string {
  switch (status) {
    case 'pending':
      return 'Queued';
    case 'in_progress':
      return 'Running';
    case 'completed':
      return 'Completed';
    case 'failed':
      return 'Failed';
    default:
      return 'Updated';
  }
}

function ToolLifecycle({ tool }: { tool: ToolCallState }) {
  const hasDetails =
    tool.rawInput !== undefined ||
    tool.rawOutput !== undefined ||
    (tool.content?.length ?? 0) > 0;
  return (
    <div className="tool-event" data-status={tool.status}>
      <div className="tool-event__summary">
        <span className="tool-event__glyph" aria-hidden="true" />
        <strong>{tool.title}</strong>
        <span>{statusLabel(tool.status)}</span>
      </div>
      {hasDetails && (
        <details>
          <summary>Details</summary>
          <pre>{JSON.stringify({ input: tool.rawInput, output: tool.rawOutput, content: tool.content }, null, 2)}</pre>
        </details>
      )}
    </div>
  );
}

export function MessageView({ message, streaming }: MessageViewProps) {
  const label = message.role === 'user' ? 'You' : message.role === 'agent' ? 'Renoa' : 'System';
  return (
    <article className="message" data-role={message.role}>
      <header>{label}</header>
      <div className="message__body">
        {message.parts.map((part, index) => {
          const key = `${message.id}-${index}`;
          if (part.type === 'content') {
            return part.content.map((block, blockIndex) => (
              <p key={`${key}-${blockIndex}`} className="message__content">
                {contentLabel(block)}
              </p>
            ));
          }
          if (part.type === 'thought') {
            return (
              <details className="reasoning" key={key} open={streaming || undefined}>
                <summary>Reasoning</summary>
                {part.thought.map((block, blockIndex) => (
                  <p key={`${key}-${blockIndex}`}>{contentLabel(block)}</p>
                ))}
              </details>
            );
          }
          if (part.type === 'tool_calls') {
            return (
              <div className="tool-events" key={key}>
                {part.toolCalls.map((tool) => <ToolLifecycle key={tool.toolCallId} tool={tool} />)}
              </div>
            );
          }
          return null;
        })}
      </div>
    </article>
  );
}
