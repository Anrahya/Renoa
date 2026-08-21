const LAST_SESSION_KEY = 'renoa.desktop.last-session';

export interface SavedSession {
  cwd: string;
  sessionId: string;
}

function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

function isSavedSession(value: unknown): value is SavedSession {
  if (!value || typeof value !== 'object') return false;
  const record = value as Record<string, unknown>;
  return typeof record.cwd === 'string' && typeof record.sessionId === 'string';
}

export function loadLastSession(): SavedSession | null {
  const raw = storage()?.getItem(LAST_SESSION_KEY);
  if (!raw) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    return isSavedSession(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

export function saveLastSession(session: SavedSession): void {
  try {
    storage()?.setItem(LAST_SESSION_KEY, JSON.stringify(session));
  } catch {
    // The ACP session remains durable even if webview presentation storage fails.
  }
}
