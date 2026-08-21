import { cleanup } from '@testing-library/react';
import { afterEach, beforeEach, vi } from 'vitest';
import { acpStore, sessionStore } from '@acp-components/core';

beforeEach(() => {
  localStorage.clear();
  acpStore.setState({
    agents: new Map(),
    workspaces: new Map(),
    activeSessionId: null,
    pendingAuth: null,
  });
  sessionStore.setState({ sessions: new Map() });
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
});
