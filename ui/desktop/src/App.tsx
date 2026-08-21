import { useMemo } from 'react';
import { acpStore, type AcpTransport } from '@acp-components/core';
import { useStore } from 'zustand/react';
import { RenoaAcpProvider, useRenoaAcp } from './acp/provider';
import { LaunchView } from './components/LaunchView';
import { WorkspaceShell } from './components/WorkspaceShell';

interface RenoaDesktopProps {
  transport: AcpTransport;
}

export function RenoaDesktop({ transport }: RenoaDesktopProps) {
  return (
    <RenoaAcpProvider transport={transport}>
      <DesktopSurface />
    </RenoaAcpProvider>
  );
}

function DesktopSurface() {
  const { agent, client, ready } = useRenoaAcp();
  const activeSessionId = useStore(acpStore, (state) => state.activeSessionId);
  const connection = agent?.status ?? 'connecting';

  const statusText = useMemo(() => {
    if (!ready || connection === 'connecting') return 'Connecting to Renoa';
    if (connection === 'connected') return 'Renoa is connected';
    if (connection === 'error') return 'Renoa could not start';
    return 'Renoa is disconnected';
  }, [connection, ready]);

  if (!activeSessionId) {
    return (
      <LaunchView
        client={client}
        connection={connection}
        ready={ready}
        statusText={statusText}
      />
    );
  }

  return (
    <WorkspaceShell
      client={client}
      connection={connection}
      sessionId={activeSessionId}
      statusText={statusText}
    />
  );
}
