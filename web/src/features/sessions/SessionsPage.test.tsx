import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { FakeEventSource } from '../../test/eventSource';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { Session } from './queries';

const MINUTE = 60_000;

function session(overrides: Partial<Session> = {}): Session {
  const now = Date.now();
  return {
    id: '0190a0b0-aaaa-7000-8000-000000000001',
    label: 'SE-001: Fix the login',
    title: 'Fix the login',
    number: 1,
    cwd: '/work/arches',
    state: 'running',
    displayState: 'running',
    listening: true,
    startedAt: now - 65 * MINUTE,
    lastEventAt: now - 5 * MINUTE,
    endedAt: null,
    runtimeMs: 65 * MINUTE,
    client: 'claude-code',
    subagents: [],
    tickets: [],
    version: 2,
    ...overrides,
  };
}

function sessionsServer(sessions: Session[]) {
  let items = sessions;
  const api = fakeApi({ 'GET /api/v1/sessions': () => json(200, { items }) });
  return {
    api,
    update(next: Session[]) {
      items = next;
    },
  };
}

describe('SessionsPage', () => {
  it('groups sessions by ticket with state, runtime, activity and running subagents', async () => {
    const now = Date.now();
    sessionsServer([
      session({
        tickets: [{ key: 'ARC-0002', title: 'Polish', status: 'in-progress' }],
        subagents: [
          { id: 'a1', type: 'Explore', state: 'running', startedAt: now - 2 * MINUTE },
          { id: 'a2', type: 'Plan', state: 'ended', startedAt: now - 9 * MINUTE, endedAt: now },
        ],
      }),
      session({
        id: 'loose-session',
        label: '',
        title: null,
        displayState: 'idle',
        state: 'idle',
        cwd: '/elsewhere',
      }),
    ]);
    renderApp('/sessions');

    const group = await screen.findByRole('region', { name: 'ARC-0002 Polish' });
    expect(within(group).getByText('SE-001: Fix the login')).toBeInTheDocument();
    expect(within(group).getByText('1 h 05 min')).toBeInTheDocument();
    expect(within(group).getByText('Last activity 5 min ago')).toBeInTheDocument();
    expect(within(group).getByText('0190a0b0')).toBeInTheDocument();
    const agents = within(group).getByRole('list', { name: 'Subagents of SE-001: Fix the login' });
    expect(within(agents).getAllByRole('listitem')).toHaveLength(1);
    expect(within(agents).getByText('Explore')).toBeInTheDocument();

    const loose = screen.getByRole('region', { name: 'Not assigned' });
    expect(within(loose).getByText('Untitled')).toBeInTheDocument();
    expect(within(loose).getByText('/elsewhere')).toBeInTheDocument();
    expect(screen.getByText('1 running')).toBeInTheDocument();
  });

  it('shows running and idle sessions first and filters by state', async () => {
    sessionsServer([
      session(),
      session({ id: 'ended-one', label: 'SE-002: Done', displayState: 'ended', state: 'ended' }),
    ]);
    renderApp('/sessions');
    expect(await screen.findByText('SE-001: Fix the login')).toBeInTheDocument();
    expect(screen.queryByText('SE-002: Done')).not.toBeInTheDocument();

    const filter = screen.getByRole('group', { name: 'Filter by state' });
    await userEvent.click(within(filter).getByRole('button', { name: 'Ended' }));
    expect(screen.getByText('SE-002: Done')).toBeInTheDocument();
    await userEvent.click(within(filter).getByRole('button', { name: 'Running' }));
    await userEvent.click(within(filter).getByRole('button', { name: 'Idle' }));
    expect(screen.queryByText('SE-001: Fix the login')).not.toBeInTheDocument();
    await userEvent.click(within(filter).getByRole('button', { name: 'Waiting for input' }));
    await userEvent.click(within(filter).getByRole('button', { name: 'Ended' }));
    expect(screen.getByText('No sessions in these states.')).toBeInTheDocument();
    await userEvent.click(within(filter).getByRole('button', { name: 'All' }));
    expect(screen.getByText('SE-001: Fix the login')).toBeInTheDocument();
    expect(screen.getByText('SE-002: Done')).toBeInTheDocument();
  });

  it('explains an empty list', async () => {
    sessionsServer([]);
    renderApp('/sessions');
    expect(await screen.findByRole('heading', { name: 'No sessions' })).toBeInTheDocument();
  });

  it('reloads when the change stream reports a session change', async () => {
    const server = sessionsServer([session()]);
    renderApp('/sessions');
    expect(await screen.findByText('SE-001: Fix the login')).toBeInTheDocument();
    await waitFor(() => {
      expect(FakeEventSource.instances).not.toHaveLength(0);
    });
    server.update([session({ displayState: 'waiting', state: 'waiting', version: 3 })]);
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        { seq: 7, entity: 'session', entityId: session().id, op: 'update', at: 2 },
        '7',
      );
    });
    expect(await screen.findByText('1 waiting for input')).toBeInTheDocument();
  });

  it('reports a list the daemon refuses', async () => {
    fakeApi({ 'GET /api/v1/sessions': () => problem(500, 'internal', 'boom') });
    renderApp('/sessions');
    expect(await screen.findByText('Could not load the sessions.')).toBeInTheDocument();
  });
});
