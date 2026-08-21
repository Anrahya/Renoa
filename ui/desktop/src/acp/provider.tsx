import {
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from 'react';
import {
  acpStore,
  createAcpProvider,
  type AcpClient,
  type AcpTransport,
  type AgentConnection,
} from '@acp-components/core';
import { useStore } from 'zustand/react';

export const RENOA_AGENT_ID = 'renoa';

interface RenoaAcpContextValue {
  agent: AgentConnection | null;
  client: AcpClient | null;
  ready: boolean;
}

const RenoaAcpContext = createContext<RenoaAcpContextValue | null>(null);

interface RenoaAcpProviderProps {
  children: ReactNode;
  transport: AcpTransport;
}

export function RenoaAcpProvider({ children, transport }: RenoaAcpProviderProps) {
  const [provider] = useState(() =>
    createAcpProvider({
      agents: [
        {
          id: RENOA_AGENT_ID,
          name: 'Renoa',
          transport: { type: 'custom', transport },
          clientInfo: { name: 'renoa-desktop', version: '0.1.0' },
        },
      ],
    }),
  );
  const ready = useSyncExternalStore(
    provider.subscribe,
    () => provider.ready,
    () => false,
  );
  const agent = useStore(acpStore, (state) => state.agents.get(RENOA_AGENT_ID) ?? null);

  useEffect(() => {
    return () => {
      provider.destroy();
    };
  }, [provider]);

  const value = useMemo<RenoaAcpContextValue>(
    () => ({ agent, client: provider.getClient(RENOA_AGENT_ID), ready }),
    [agent, provider, ready],
  );

  return <RenoaAcpContext.Provider value={value}>{children}</RenoaAcpContext.Provider>;
}

export function useRenoaAcp(): RenoaAcpContextValue {
  const value = useContext(RenoaAcpContext);
  if (!value) throw new Error('useRenoaAcp must be used inside RenoaAcpProvider');
  return value;
}
