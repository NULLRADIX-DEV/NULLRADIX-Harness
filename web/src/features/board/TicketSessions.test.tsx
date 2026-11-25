import { act, screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { boardServer } from '../../test/boardServer';
import { json } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';

const KEY = 'ARC-0001';

async function openPanel() {
  const backlog = await screen.findByRole('region', { name: 'Backlog' });
  await userEvent.click(await within(backlog).findByRole('button', { name: /ARC-0001/ }));
  return within(await screen.findByRole('dialog', { name: `Ticket ${KEY}` }));
}

describe('TicketSessions', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('turns a silent session stale while the panel stays open', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const server = boardServer();
    server.api.set(`GET /api/v1/tickets/${KEY}/sessions`, () =>
      json(200, {
        items: [
          {
            id: 's-2',
            label: 'SE-004: Keep going',
            cwd: '/repo',
            state: 'running',
            displayState: 'running',
            listening: true,
            startedAt: 0,
            lastEventAt: Date.now() - 29 * 60_000,
            runtimeMs: 0,
            client: 'claude-code',
            subagents: [],
            tickets: [],
            version: 2,
          },
        ],
      }),
    );
    renderApp('/board');
    const panel = await openPanel();
    const sessions = await panel.findByRole('region', { name: 'Sessions' });
    expect(within(sessions).getByText('Running')).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(2 * 60_000);
    });
    expect(within(sessions).getByText('Stale')).toBeInTheDocument();
  });

  it('lists the sessions that worked on the ticket', async () => {
    const server = boardServer();
    server.api.set(`GET /api/v1/tickets/${KEY}/sessions`, () =>
      json(200, {
        items: [
          {
            id: 's-1',
            label: 'SE-003: Fix the login',
            cwd: '/repo',
            state: 'ended',
            displayState: 'ended',
            listening: false,
            startedAt: 0,
            lastEventAt: Date.now(),
            endedAt: Date.now(),
            runtimeMs: 0,
            client: 'claude-code',
            subagents: [],
            tickets: [{ key: KEY, title: 'Show the board', status: 'backlog' }],
            version: 3,
          },
        ],
      }),
    );
    renderApp('/board');
    const panel = await openPanel();
    const sessions = await panel.findByRole('region', { name: 'Sessions' });
    expect(within(sessions).getByText('SE-003: Fix the login')).toBeInTheDocument();
    expect(within(sessions).getByText('Ended')).toBeInTheDocument();
    expect(within(sessions).getByText('just now')).toBeInTheDocument();
  });

  it('stays hidden without sessions', async () => {
    boardServer();
    renderApp('/board');
    const panel = await openPanel();
    expect(panel.queryByRole('region', { name: 'Sessions' })).not.toBeInTheDocument();
  });
});
