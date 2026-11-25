import { screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
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
