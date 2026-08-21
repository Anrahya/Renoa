import type { AnyMessage } from '@acp-components/core';
import { waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { TauriAcpTransport } from './tauriTransport';

interface MockEvent {
  payload: {
    bridgeId: string;
    data?: string;
    message?: string;
  };
}

type MockListener = (event: MockEvent) => void;

const native = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  listeners: new Map<string, MockListener>(),
  operations: [] as string[],
}));

vi.mock('@tauri-apps/api/core', () => ({ invoke: native.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));

beforeEach(() => {
  native.invoke.mockReset();
  native.listen.mockReset();
  native.listeners.clear();
  native.operations.length = 0;
  native.invoke.mockImplementation(async (command: string) => {
    native.operations.push(`invoke:${command}`);
  });
  native.listen.mockImplementation(async (event: string, listener: MockListener) => {
    native.operations.push(`listen:${event}`);
    native.listeners.set(event, listener);
    return () => {
      native.listeners.delete(event);
    };
  });
});

describe('Tauri ACP transport', () => {
  it('registers listeners before launch and gives every prompt one stable turn identity', async () => {
    const transport = new TauriAcpTransport('bridge-1');
    const stream = await transport.connect();

    expect(native.operations.slice(0, 5)).toEqual([
      'listen:renoa-acp-output',
      'listen:renoa-acp-closed',
      'listen:renoa-acp-error',
      'listen:renoa-acp-diagnostic',
      'invoke:start_agent',
    ]);

    const writer = stream.writable.getWriter();
    await writer.write({
      jsonrpc: '2.0',
      id: 7,
      method: 'session/prompt',
      params: {
        sessionId: 'session-1',
        prompt: [{ type: 'text', text: 'Hello' }],
      },
    } as unknown as AnyMessage);

    const writeCall = native.invoke.mock.calls.find(([command]) => command === 'write_to_agent');
    const invocation = writeCall?.[1] as {
      args: { bridgeId: string; line: string };
    };
    const wire = JSON.parse(invocation.args.line) as {
      params: { _meta: { requestId: string; promptId: string } };
    };
    expect(invocation.args.bridgeId).toBe('bridge-1');
    expect(wire.params._meta.requestId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    expect(wire.params._meta.promptId).toBe(wire.params._meta.requestId);

    await writer.close();
    expect(native.operations).toContain('invoke:kill_agent');
  });

  it('finishes an old child shutdown before reconnecting', async () => {
    let finishKill!: () => void;
    const blockedKill = new Promise<void>((resolve) => {
      finishKill = resolve;
    });
    native.invoke.mockImplementation(async (command: string) => {
      native.operations.push(`invoke:${command}`);
      if (command === 'kill_agent') await blockedKill;
    });

    const transport = new TauriAcpTransport('bridge-1');
    await transport.connect();
    transport.disconnect();
    const reconnected = transport.connect();

    await waitFor(() => {
      expect(native.operations).toContain('invoke:kill_agent');
    });
    expect(native.operations.filter((operation) => operation === 'invoke:start_agent')).toHaveLength(
      1,
    );

    finishKill();
    const stream = await reconnected;
    expect(native.operations.filter((operation) => operation === 'invoke:start_agent')).toHaveLength(
      2,
    );

    native.invoke.mockResolvedValue(undefined);
    await stream.writable.getWriter().close();
  });
});
