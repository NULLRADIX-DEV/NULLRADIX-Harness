import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { fakeApi, json } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { WORK_ID, dispatchRun, runDetail, transcriptLine } from '../../test/runFixtures';
import type { Run } from '../runs/queries';
import type { SessionDetail } from './queries';

const ID = 'claude-session-1';

function detail(runName: string | null): SessionDetail {
  const now = Date.now();
  return {
    session: {
      id: ID,
      label: 'SE-001: Fix the login',
      title: 'Fix the login',
      number: 1,
      cwd: '/work/arches',
      state: 'ended',
      displayState: 'ended',
      listening: false,
      startedAt: now - 60_000,
      lastEventAt: now,
      endedAt: now,
      runtimeMs: 60_000,
      client: 'nullradix',
      subagents: [],
      tickets: [],
      runName,
      version: 1,
    },
    client: 'nullradix',
    transcript: { available: true, path: '/t/s.jsonl', sizeBytes: 1 },
    tokens: { input: 1, output: 2, cacheRead: 3, cacheWrite: 4 },
    durationMs: null,
    todos: [],
  };
}

function serve(runName: string | null, runs: Run[]) {
  return fakeApi({
    [`GET /api/v1/sessions/${ID}/detail`]: () => json(200, detail(runName)),
    [`GET /api/v1/sessions/${ID}/transcript`]: () =>
      json(200, { entries: [], cursor: 0, skipped: 0, available: true }),
    'GET /api/v1/runs': () => json(200, { items: runs }),
    [`GET /api/v1/runs/${WORK_ID}`]: () => json(200, runDetail()),
    [`GET /api/v1/runs/${WORK_ID}/transcript`]: () =>
      json(200, {
        lines: [transcriptLine(1, 'say', 'Working on it')],
        lastSeq: 1,
        finished: true,
      }),
  });
}

describe('session Run tab', () => {
  it('shows the run that started the session next to the other tabs', async () => {
    const api = serve(WORK_ID, [dispatchRun()]);
    renderApp(`/sessions/${ID}`);
    await userEvent.click(await screen.findByRole('tab', { name: 'Run' }));
    const panel = screen.getByRole('tabpanel');
    expect(await within(panel).findByText('Working on it')).toBeInTheDocument();
    expect(within(panel).getByText('Succeeded')).toBeInTheDocument();
    const query = new URL(api.requests('GET /api/v1/runs')[0]?.url ?? '').searchParams;
    expect(query.get('sessionId')).toBe(ID);
    expect(query.get('runId')).toBe(WORK_ID);
  });

  it('has no Run tab when the session has no run name', async () => {
    const api = serve(null, [dispatchRun()]);
    renderApp(`/sessions/${ID}`);
    await screen.findByRole('tab', { name: 'Changes' });
    expect(screen.queryByRole('tab', { name: 'Run' })).not.toBeInTheDocument();
    expect(api.requests('GET /api/v1/runs')).toHaveLength(0);
  });

  it('has no Run tab when no run belongs to the session', async () => {
    const api = serve(WORK_ID, []);
    renderApp(`/sessions/${ID}`);
    await screen.findByRole('tab', { name: 'Changes' });
    await waitFor(() => {
      expect(api.requests('GET /api/v1/runs')).toHaveLength(1);
    });
    expect(screen.queryByRole('tab', { name: 'Run' })).not.toBeInTheDocument();
  });
});
