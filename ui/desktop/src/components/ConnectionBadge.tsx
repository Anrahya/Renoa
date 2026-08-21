import type { ConnectionStatus } from '@acp-components/core';

interface ConnectionBadgeProps {
  status: ConnectionStatus;
  text: string;
}

export function ConnectionBadge({ status, text }: ConnectionBadgeProps) {
  return (
    <div className="connection-badge" data-status={status} role="status">
      <span className="connection-badge__indicator" aria-hidden="true" />
      <span>{text}</span>
    </div>
  );
}
