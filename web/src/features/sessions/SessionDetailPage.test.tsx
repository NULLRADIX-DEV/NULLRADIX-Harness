import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { fakeApi, json } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { SessionDetail, TranscriptEntry, TranscriptPage } from './queries';
import { mergeEntries } from './transcript';
import { toolDetail, toolName } from './TranscriptView';

const ID = 's-1';

function detail(overrides: Partial<SessionDetail> = {}): SessionDetail {
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
      model: 'claude-x',
      client: 'claude-code',
      subagents: [],
      tickets: [{ key: 'ARC-0001', title: 'Login', status: 'in-progress' }],
      version: 3,
    },
    client: 'claude-desktop',
    transcript: { available: true, path: '/t/s-1.jsonl', sizeBytes: 100 },
    branch: 'feature/login',
    claudeVersion: '2.1.0',
    tokens: { input: 1, output: 2, cacheRead: 3, cacheWrite: 4 },
    costUsd: 0.42,
    durationMs: null,
    context: { usedTokens: 1205, limitTokens: 200_000 },
    todos: [{ content: 'Write tests', status: 'in-progress' }],
    ...overrides,
  };
}

const ENTRIES: TranscriptEntry[] = [
  { kind: 'prompt', id: 'u1', at: '2026-11-04T08:00:00Z', text: 'Please fix the login' },
  { kind: 'thinking', id: 'a1:0', at: '2026-11-04T08:00:01Z', text: '', durationMs: 2000 },
  {
    kind: 'tool',
    id: 'toolu_1',
    at: '2026-11-04T08:00:02Z',
    name: 'Bash',
    summary: 'Bash: cargo test',
    input: { command: 'cargo test', description: 'Run the tests' },
    result: { text: 'Exit code 1', isError: true, truncated: false },
  },
  { kind: 'text', id: 'a2:0', at: '2026-11-04T08:00:03Z', text: 'The tests fail.' },
  { kind: 'system', id: 's1', at: '2026-11-04T08:00:04Z', text: 'Hook failed', level: 'warning' },
];

function server(page: Partial<TranscriptPage> = {}) {
  return fakeApi({
    [`GET /api/v1/sessions/${ID}/detail`]: () => json(200, detail()),
    [`GET /api/v1/sessions/${ID}/transcript`]: () =>
      json(200, { entries: ENTRIES, cursor: 500, skipped: 1, available: true, ...page }),
    [`GET /api/v1/sessions/${ID}/changes`]: () =>
      json(200, {
        origin: 'git',
        base: 'main',
        note: null,
        files: [{ path: 'src/login.rs', status: 'modified', added: 2, removed: 1 }],
      }),
    [`GET /api/v1/sessions/${ID}/changes/file`]: () =>
      json(200, {
        path: 'src/login.rs',
        truncated: false,
        hunks: [
          {
            header: '@@ -3,2 +3,3 @@',
            lines: [
              { kind: 'context', text: 'fn login() {' },
              { kind: 'remove', text: '    todo!()' },
              { kind: 'add', text: '    check()' },
              { kind: 'add', text: '    ok()' },
            ],
          },
        ],
      }),
  });
}

describe('SessionDetailPage', () => {
  it('shows the figures and the transcript of the session', async () => {
    const api = server();
    renderApp(`/sessions/${ID}`);
    expect(
      await screen.findByRole('heading', { name: 'SE-001: Fix the login' }),
    ).toBeInTheDocument();
    expect(screen.getByText('Desktop')).toBeInTheDocument();
    expect(screen.getByText('feature/login')).toBeInTheDocument();
    expect(screen.getByText('$0.42')).toBeInTheDocument();
    expect(screen.getByText('1205 of 200000')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'ARC-0001' })).toHaveAttribute(
      'href',
      '/board?ticket=ARC-0001',
    );
    const todos = screen.getByRole('region', { name: 'Todos' });
    expect(within(todos).getByText('Write tests')).toBeInTheDocument();

    const transcript = await screen.findByLabelText('Transcript');
    await within(transcript).findByText('Please fix the login');
    expect(within(transcript).getByText('Please fix the login')).toBeInTheDocument();
    expect(within(transcript).getByText('Thought for 2 s')).toBeInTheDocument();
    expect(within(transcript).getByText('The tests fail.')).toBeInTheDocument();
    expect(within(transcript).getByText('Hook failed')).toBeInTheDocument();
    expect(
      within(transcript).getByText('One line of the transcript could not be read.'),
    ).toBeInTheDocument();
    const tool = within(transcript).getByRole('button', { name: /Bash/ });
    expect(within(tool).getByText('Failed')).toBeInTheDocument();
    await userEvent.click(tool);
    expect(within(transcript).getByLabelText('Command')).toHaveTextContent('cargo test');
    expect(within(transcript).getByText('Run the tests')).toBeInTheDocument();
    const [request] = api.requests(`GET /api/v1/sessions/${ID}/transcript`);
    expect(new URL(request?.url ?? '').searchParams.get('after')).toBe('0');
  });

  it('lists the changed files and opens the diff of one', async () => {
    server();
    renderApp(`/sessions/${ID}`);
    await userEvent.click(await screen.findByRole('tab', { name: 'Changes' }));
    expect(await screen.findByText(/From git against/)).toBeInTheDocument();
    const list = screen.getByRole('list', { name: 'Changed files' });
    await userEvent.click(within(list).getByRole('button', { name: /login\.rs/ }));
    const diff = await screen.findByLabelText('Diff of src/login.rs');
    expect(within(diff).getByText('@@ -3,2 +3,3 @@')).toBeInTheDocument();
    expect(
      within(diff).getByText('check()', { normalizer: (text) => text.trim() }),
    ).toBeInTheDocument();
  });

  it('says so when the transcript is not available', async () => {
    server({ entries: [], available: false });
    renderApp(`/sessions/${ID}`);
    expect(await screen.findByRole('heading', { name: 'No transcript' })).toBeInTheDocument();
  });

  it('reads the transcript again when the detail changes', async () => {
    const api = server();
    renderApp(`/sessions/${ID}`);
    await screen.findByText('Please fix the login');
    await waitFor(() => {
      expect(api.requests(`GET /api/v1/sessions/${ID}/transcript`)).toHaveLength(1);
    });
  });
});

describe('transcript helpers', () => {
  it('replaces entries whose id comes again', () => {
    const tool = (text: string | null): TranscriptEntry => ({
      kind: 'tool',
      id: 'toolu_1',
      at: '',
      name: 'Bash',
      summary: 'Bash: ls',
      input: { command: 'ls' },
      result: text === null ? null : { text, isError: false, truncated: false },
    });
    const prompt: TranscriptEntry = { kind: 'prompt', id: 'u1', at: '', text: 'Go' };
    const done = tool('a.txt');
    const merged = mergeEntries([prompt, tool(null)], [done]);
    expect(merged.map((entry) => entry.id)).toEqual(['u1', 'toolu_1']);
    expect(merged[1]).toBe(done);
  });

  it('names tools short', () => {
    expect(toolName('mcp__claude-in-chrome__navigate')).toBe('claude-in-chrome \u00b7 navigate');
    expect(toolDetail('Bash', 'Bash: npm test')).toBe('npm test');
    expect(toolDetail('Read', 'Read')).toBe('');
  });
});
