import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { boardServer } from '../../test/boardServer';
import { configView, statusDto } from '../../test/dispatcherFixture';
import { FakeEventSource } from '../../test/eventSource';
import { json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { InboxItem } from '../inbox/queries';
import type { DispatchStats } from './queries';

const STATS = '/api/v1/stats/dispatch';
const CHANGED_AT = Date.UTC(2026, 10, 20, 12);

function statsDto(overrides: Partial<DispatchStats> = {}): DispatchStats {
  return {
    filter: { boardId: null, from: '2026-10-02', to: '2026-11-26', kind: 'work' },
    configChangedAt: CHANGED_AT,
    kpis: {
      runs: 12,
      subjects: 8,
      done: 7,
      aborted: 4,
      solved: 6,
      successRate: 0.75,
      firstTryRate: 0.5,
      escalationRate: 0.375,
      finalFailures: 2,
    },
    usage: {
      costUsd: 12.345,
      inputTokens: 12_300,
      outputTokens: 4_100_000,
      cacheReadTokens: 950,
      cacheWriteTokens: 0,
      averageDurationMs: 45 * 60_000,
    },
    series: {
      unit: 'week',
      points: [
        { start: '2026-11-09', done: 3, aborted: 1, other: 0 },
        { start: '2026-11-16', done: 4, aborted: 3, other: 1 },
      ],
    },
    byProfile: [
      { profile: 'medium', runs: 4, done: 1, escalated: 1, final: 2, other: 0 },
      { profile: 'small', runs: 8, done: 6, escalated: 2, final: 0, other: 0 },
    ],
    ladder: { finished: 8, firstTry: 0.5, afterEscalation: 0.25, final: 0.25 },
    abortReasons: [
      { category: 'max-turns', label: 'Turn limit', count: 3 },
      { category: 'other', label: 'Other', count: 1 },
    ],
    configurableAbortShare: 0.75,
    outcomes: [
      { outcome: 'done', count: 7 },
      { outcome: 'abort', count: 3 },
    ],
    usagePauses: 2,
    staleRuns: 1,
    ...overrides,
  };
}

function emptyStats(overrides: Partial<DispatchStats> = {}): DispatchStats {
  return statsDto({
    kpis: {
      runs: 0,
      subjects: 0,
      done: 0,
      aborted: 0,
      solved: 0,
      successRate: null,
      firstTryRate: null,
      escalationRate: null,
      finalFailures: 0,
    },
    usage: {
      costUsd: 0,
      inputTokens: 0,
      outputTokens: 0,
      cacheReadTokens: 0,
      cacheWriteTokens: 0,
      averageDurationMs: null,
    },
    series: { unit: 'day', points: [{ start: '2026-11-26', done: 0, aborted: 0, other: 0 }] },
    byProfile: [],
    ladder: { finished: 0, firstTry: null, afterEscalation: null, final: null },
    abortReasons: [],
    configurableAbortShare: null,
    outcomes: [],
    usagePauses: 0,
    staleRuns: 0,
    ...overrides,
  });
}

const BLOCKED: InboxItem = {
  type: 'ticket',
  key: 'ARC-0001',
  kind: 'merge',
  title: 'Merge worktrees',
  detail: 'conflict in a.rs',
  boardId: null,
  boardName: null,
  projectKey: 'ARC',
  waitingTickets: 0,
  priority: 'normal',
  sinceAt: 1,
  rank: 1,
};

const QUESTION: InboxItem = {
  ...BLOCKED,
  type: 'question',
  key: 'ARC-Q0001',
  kind: 'question',
  title: 'Which store?',
  detail: null,
  waitingTickets: 2,
  rank: 3,
};

function server(
  options: {
    stats?: (query: URLSearchParams) => Response;
    items?: InboxItem[];
  } = {},
) {
  const { api } = boardServer();
  const items = options.items ?? [BLOCKED, QUESTION];
  api.set(`GET ${STATS}`, (request) =>
    (options.stats ?? (() => json(200, statsDto())))(new URL(request.url).searchParams),
  );
  api.set('GET /api/v1/dispatcher', () =>
    json(
      200,
      statusDto({
        tickets: [
          { key: 'ARC-0005', title: 'Wait', reason: 'waits for ARC-0002, ARC-0003' },
          { key: 'ARC-0006', title: 'Merge', reason: 'waits for the merge of ARC-0004' },
          { key: 'ARC-0007', title: 'Off', reason: 'label "manual"' },
          { key: 'ARC-0008', title: 'Ready', reason: 'ready' },
        ],
      }),
    ),
  );
  api.set('GET /api/v1/dispatcher/config', () => json(200, configView()));
  api.set('GET /api/v1/me/inbox', () =>
    json(200, {
      items,
      counts: { merge: 1, setup: 0, question: 1, accept: 0, blocked: 0, planning: 0, note: 0 },
      total: items.length,
      orderVersion: 1,
    }),
  );
  api.set('GET /api/v1/runs', () => json(200, { items: [] }));
  return api;
}

function lastQuery(api: ReturnType<typeof server>): URLSearchParams {
  const request = api.requests(`GET ${STATS}`).at(-1);
  if (!request) throw new Error('no stats request');
  return new URL(request.url).searchParams;
}

describe('DashboardPage', () => {
  it('shows the numbers, the chart, the ladder and the obstacles', async () => {
    const api = server();
    renderApp('/dashboard');
    expect(await screen.findByRole('heading', { name: 'Runs per week' })).toBeInTheDocument();

    const kpi = (name: string) => {
      const tile = screen
        .getAllByRole('region', { name })
        .find((region) => region.classList.contains('kpi'));
      if (!tile) throw new Error('no tile ' + name);
      return tile;
    };
    expect(within(kpi('Runs')).getByText('12')).toBeInTheDocument();
    expect(within(kpi('Success rate')).getByText('75 %')).toBeInTheDocument();
    expect(within(kpi('First-try rate')).getByText('50 %')).toBeInTheDocument();
    expect(within(kpi('Escalation rate')).getByText('38 %')).toHaveClass('tone-warn');
    expect(within(kpi('Failed for good')).getByText('2')).toHaveClass('tone-bad');

    // the chart has a text alternative with every bucket
    const table = screen.getByRole('table', { name: 'Runs per week' });
    expect(
      within(table).getByRole('row', { name: /16 Nov - 22 Nov\s+4\s+3\s+1/ }),
    ).toBeInTheDocument();

    // ladder in the configured order, small first, with the share solved
    const ladder = screen.getByRole('region', { name: 'Escalation along the ladder' });
    const names = within(ladder)
      .getAllByRole('listitem')
      .map((item) => item.querySelector('.ladder-name')?.textContent);
    expect(names).toEqual(['small', 'medium', 'large']);
    expect(within(ladder).getByText('75 %')).toBeInTheDocument();
    expect(within(ladder).getByText('solved after an escalation')).toBeInTheDocument();

    // abort reasons with their share of the aborts
    const reasons = screen.getByRole('region', { name: 'Most frequent abort reasons' });
    expect(within(reasons).getByText('Turn limit')).toBeInTheDocument();
    expect(within(reasons).getByText('75 %')).toBeInTheDocument();

    const obstacles = screen.getByRole('region', { name: 'Obstacles' });
    expect(
      await within(obstacles).findByText('planned, waiting for ARC-0002, ARC-0003, ARC-0004'),
    ).toBeInTheDocument();
    expect(within(obstacles).getByText('label "manual"')).toBeInTheDocument();
    expect(
      within(obstacles).getByText('Usage limit pauses').parentElement?.previousElementSibling,
    ).toHaveTextContent('2');

    // usage and outcomes
    const usage = screen.getByRole('region', { name: 'Usage and outcomes' });
    expect(within(usage).getByText('$12.35')).toBeInTheDocument();
    expect(within(usage).getByText('45 min')).toBeInTheDocument();
    expect(within(usage).getByText('4.1M')).toBeInTheDocument();
    expect(within(usage).getByText('Aborted')).toBeInTheDocument();

    // the default range is 56 days up to today
    const query = lastQuery(api);
    expect(query.get('kind')).toBe('work');
    expect(query.get('from')).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    expect(query.has('boardId')).toBe(false);
  });

  it('lists what waits for the person with actions', async () => {
    server();
    renderApp('/dashboard');
    const blocked = await screen.findByRole('list', { name: 'Blocked' });
    expect(within(blocked).getByRole('link', { name: 'Merge worktrees' })).toHaveAttribute(
      'href',
      '/board?ticket=ARC-0001',
    );
    expect(
      within(blocked).getByRole('img', { name: 'Waiting for over 24 hours' }),
    ).toBeInTheDocument();
    const questions = screen.getByRole('list', { name: 'Questions' });
    expect(within(questions).getByText(/2 tickets wait/)).toBeInTheDocument();
    expect(within(questions).getByRole('link', { name: 'Answer ARC-Q0001' })).toBeInTheDocument();
  });

  it('tells when nothing ran in the range and has nothing to do', async () => {
    server({ stats: () => json(200, emptyStats()), items: [] });
    renderApp('/dashboard');
    expect(await screen.findByRole('note')).toHaveTextContent('No work runs between');
    expect(screen.getAllByText('-').length).toBeGreaterThan(2);
    expect(
      screen.getByText('No runs in the range.', { selector: '.dash-empty' }),
    ).toBeInTheDocument();
    expect(screen.getByText('No aborts in the range.')).toBeInTheDocument();
    expect(await screen.findByText(/Nothing to do/)).toBeInTheDocument();
  });

  it('switches the kind and asks for it', async () => {
    const api = server();
    renderApp('/dashboard');
    await screen.findByRole('heading', { name: 'Runs per week' });
    await userEvent.click(screen.getByRole('button', { name: 'Research' }));
    await waitFor(() => {
      expect(lastQuery(api).get('kind')).toBe('research');
    });
    expect(screen.getByRole('button', { name: 'Research' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    expect(screen.getByRole('button', { name: 'Work' })).toHaveAttribute('aria-pressed', 'false');
    await userEvent.click(screen.getByRole('button', { name: 'Merge' }));
    await waitFor(() => {
      expect(lastQuery(api).get('kind')).toBe('merge');
    });
  });

  it('filters by board and reads the filter from the address', async () => {
    const api = server();
    renderApp('/dashboard?kind=merge&period=7');
    await screen.findByRole('heading', { name: 'Runs per week' });
    expect(screen.getByRole('button', { name: 'Merge' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('combobox', { name: 'Range' })).toHaveValue('7');
    const first = lastQuery(api);
    expect(first.get('kind')).toBe('merge');
    const days =
      (Date.parse(`${first.get('to') ?? ''}T00:00:00Z`) -
        Date.parse(`${first.get('from') ?? ''}T00:00:00Z`)) /
      86_400_000;
    expect(days).toBe(6);

    const board = screen.getByRole('combobox', { name: 'Board' });
    const option = await within(board).findByRole('option', { name: /ARC \// });
    await userEvent.selectOptions(board, option);
    await waitFor(() => {
      expect(lastQuery(api).get('boardId')).not.toBeNull();
    });
  });

  it('enables "since config change" from the date the stats return', async () => {
    const api = server();
    renderApp('/dashboard');
    const option = await screen.findByRole('option', {
      name: /Since config change \(20 Nov 2026\)/,
    });
    expect(option).toBeEnabled();
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Range' }), option);
    await waitFor(() => {
      expect(lastQuery(api).get('from')).toBe('config');
    });
  });

  it('disables "since config change" while no change is known', async () => {
    server({ stats: () => json(200, statsDto({ configChangedAt: null })) });
    renderApp('/dashboard');
    const option = await screen.findByRole('option', { name: /Since config change \(none yet\)/ });
    expect(option).toBeDisabled();
  });

  it('takes a custom range and pulls the other edge along', async () => {
    const api = server();
    renderApp('/dashboard');
    await screen.findByRole('heading', { name: 'Runs per week' });
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Range' }), 'custom');
    // starts with the range shown before
    const from = await screen.findByLabelText('From');
    const to = screen.getByLabelText('To');
    expect(from).toHaveValue('2026-10-02');
    expect(to).toHaveValue('2026-11-26');
    await waitFor(() => {
      expect(lastQuery(api).get('from')).toBe('2026-10-02');
    });

    fireEvent.change(from, { target: { value: '2026-11-10' } });
    await waitFor(() => {
      expect(lastQuery(api).get('from')).toBe('2026-11-10');
    });
    expect(lastQuery(api).get('to')).toBe('2026-11-26');
  });

  it('shows the daemon text when the range is refused', async () => {
    server({
      stats: () => problem(422, 'stats-range-invalid', 'a range has at most 366 days'),
    });
    renderApp('/dashboard');
    expect(await screen.findByText('a range has at most 366 days')).toBeInTheDocument();
  });

  it('refetches when a run changes', async () => {
    const api = server();
    renderApp('/dashboard');
    await screen.findByRole('heading', { name: 'Runs per week' });
    await waitFor(() => {
      expect(FakeEventSource.instances).not.toHaveLength(0);
    });
    const before = api.requests(`GET ${STATS}`).length;
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        { seq: 1, entity: 'run', entityId: 'x', op: 'update', at: 2 },
        '1',
      );
    });
    await waitFor(() => {
      expect(api.requests(`GET ${STATS}`).length).toBeGreaterThan(before);
    });
  });
});
