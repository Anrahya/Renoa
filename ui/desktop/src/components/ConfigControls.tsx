import { useState } from 'react';
import {
  setSessionConfigOption,
  sessionStore,
  type AcpClient,
  type SessionConfigOption,
} from '@acp-components/core';
import { useStore } from 'zustand/react';

interface ConfigControlsProps {
  client: AcpClient | null;
  disabled: boolean;
  sessionId: string;
}

interface FlatOption {
  label: string;
  value: string;
}

function flattenOptions(option: Extract<SessionConfigOption, { type: 'select' }>): FlatOption[] {
  const values: FlatOption[] = [];
  for (const candidate of option.options) {
    if ('options' in candidate) {
      for (const child of candidate.options) {
        values.push({ label: child.name, value: child.value });
      }
    } else {
      values.push({ label: candidate.name, value: candidate.value });
    }
  }
  return values;
}

export function ConfigControls({ client, disabled, sessionId }: ConfigControlsProps) {
  const configOptions = useStore(
    sessionStore,
    (state) => state.sessions.get(sessionId)?.configOptions ?? [],
  );
  const [changing, setChanging] = useState<string | null>(null);
  const visible = configOptions.filter(
    (option): option is Extract<SessionConfigOption, { type: 'select' }> => option.type === 'select',
  );

  if (visible.length === 0) return null;

  const change = async (configId: string, value: string) => {
    if (!client) return;
    setChanging(configId);
    try {
      await setSessionConfigOption(client, sessionId, configId, value);
    } finally {
      setChanging(null);
    }
  };

  return (
    <div className="config-controls" aria-label="Session configuration">
      {visible.map((option) => (
        <label className="compact-select" key={option.id}>
          <span>{option.name}</span>
          <select
            aria-label={option.name}
            value={String(option.currentValue)}
            disabled={disabled || changing !== null || !client}
            onChange={(event) => void change(option.id, event.target.value)}
          >
            {flattenOptions(option).map((value) => (
              <option key={value.value} value={value.value}>{value.label}</option>
            ))}
          </select>
        </label>
      ))}
    </div>
  );
}
