import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { RenoaDesktop } from './App';
import { FixtureAcpTransport } from './test/fixtureTransport';

async function startSession(transport: FixtureAcpTransport) {
  const user = userEvent.setup();
  render(<RenoaDesktop transport={transport} />);
  await screen.findByText('Renoa is connected');
  await user.type(screen.getByLabelText('Workspace'), '/workspace/renoa');
  await user.click(screen.getByRole('button', { name: 'Start session' }));
  await screen.findByText('What should Renoa work on?');
  return user;
}

describe('Renoa desktop ACP surface', () => {
  it('creates a session, changes configuration, and renders the streamed turn', async () => {
    const transport = new FixtureAcpTransport();
    const user = await startSession(transport);

    await user.selectOptions(screen.getByLabelText('Model'), 'fixture-fast');
    await waitFor(() => {
      expect(transport.requests).toContain('session/set_config_option');
    });

    await user.type(screen.getByLabelText('Prompt'), 'Inspect value.txt');
    await user.click(screen.getByRole('button', { name: 'Send' }));

    expect(await screen.findByText('Inspect value.txt')).toBeTruthy();
    expect(await screen.findByText('Inspecting the workspace before answering.')).toBeTruthy();
    expect(await screen.findByText('Read workspace file')).toBeTruthy();
    expect(await screen.findByText('Completed')).toBeTruthy();
    expect(await screen.findByText('The requested file is ready.')).toBeTruthy();
    expect(transport.requests).toContain('session/prompt');
  });

  it('cancels an active ACP prompt', async () => {
    const transport = new FixtureAcpTransport();
    const user = await startSession(transport);

    await user.type(screen.getByLabelText('Prompt'), 'Wait for cancellation');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await user.click(await screen.findByRole('button', { name: 'Stop' }));

    await waitFor(() => {
      expect(transport.requests).toContain('session/cancel');
      expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy();
    });
  });

  it('loads transcript history from Renoa instead of browser storage', async () => {
    const transport = new FixtureAcpTransport();
    localStorage.setItem(
      'renoa.desktop.last-session',
      JSON.stringify({ cwd: '/workspace/renoa', sessionId: transport.sessionId }),
    );
    localStorage.setItem(
      `renoa.desktop.history.${transport.sessionId}`,
      JSON.stringify([
        {
          id: 'saved-answer',
          role: 'agent',
          parts: [{ type: 'content', content: [{ type: 'text', text: 'Saved answer' }] }],
          timestamp: 1,
        },
      ]),
    );

    const user = userEvent.setup();
    render(<RenoaDesktop transport={transport} />);
    await screen.findByText('Renoa is connected');
    await user.click(screen.getByRole('button', { name: 'Resume session' }));

    expect(await screen.findByText('Durable question')).toBeTruthy();
    expect(await screen.findByText('Durable answer')).toBeTruthy();
    expect(screen.queryByText('Saved answer')).toBeNull();
    expect(transport.requests).toContain('session/load');
  });
});
