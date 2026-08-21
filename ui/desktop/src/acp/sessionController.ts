import {
  acpStore,
  createSession,
  loadSession,
  sessionStore,
  type AcpClient,
} from '@acp-components/core';
import { RENOA_AGENT_ID } from './provider';
import { saveLastSession } from './persistence';

function bindWorkspace(cwd: string): void {
  acpStore.getState().addWorkspace(cwd);
}

export async function createRenoaSession(client: AcpClient, cwd: string): Promise<string> {
  bindWorkspace(cwd);
  const sessionId = await createSession(client, RENOA_AGENT_ID, cwd);
  acpStore.getState().setActiveSession(sessionId);
  saveLastSession({ cwd, sessionId });
  return sessionId;
}

export async function loadRenoaSession(
  client: AcpClient,
  sessionId: string,
  cwd: string,
): Promise<void> {
  bindWorkspace(cwd);
  acpStore.getState().addSession({
    id: sessionId,
    cwd,
    agentId: RENOA_AGENT_ID,
    loaded: false,
  });
  sessionStore.getState().ensureSession(sessionId);
  try {
    await loadSession(client, sessionId, cwd);
    acpStore.getState().setActiveSession(sessionId);
    saveLastSession({ cwd, sessionId });
  } catch (error) {
    acpStore.getState().removeSession(sessionId);
    sessionStore.getState().removeSession(sessionId);
    throw error;
  }
}
