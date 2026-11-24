import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { FakeEventSource } from '../../test/eventSource';
import { fakeApi, json, problem, type Handler } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { FINISHED_MESSAGE, GONE_MESSAGE } from './RunsSection';
import { OUTPUT_PAGE, OUTPUT_POLL_MS, type OutputLine, type Run } from './queries';

const BUILD_ID = '0190a0b0-0000-7000-8000-0000000000a1';
const TEST_ID = '0190a0b0-0000-7000-8000-0000000000a2';

interface Page {
  lines: OutputLine[];
  finished: boolean;
}

function run(overrides: Partial<Run> = {}): Run {
  return {
    id: BUILD_ID,
    label: 'cargo build',
    status: 'running',
    createdAt: 1_000,
    startedAt: 1_000,
    version: 2,
    ...overrides,
  };
}

function line(seq: number, text: string, stream: OutputLine['stream'] = 'stdout'): OutputLine {
  return { seq, stream, text };
}

// Serves the pages of one run in order, then an empty unfinished page.
function pages(...served: Page[]): Handler {
  const queue = [...served];
  return () => json(200, queue.shift() ?? { lines: [], finished: false });
}

function outputRoute(id: string) {
  return `GET /api/v1/runs/${id}/output`;
}

// In-memory daemon for the runs routes.
function runsServer(runs: Run[], output: Record<string, Handler> = {}) {
  let items = runs;
  const api = fakeApi({
    'GET /api/v1/runs': () => json(200, { items }),
    [`POST /api/v1/runs/${BUILD_ID}/cancel`]: () => json(202, items[0]),
    ...Object.fromEntries(
      Object.entries(output).map(([id, handler]) => [outputRoute(id), handler]),
    ),
  });
  return {
    api,
    update(next: Run[]) {
      items = next;
    },
  };
}

function runList() {
  return screen.findByRole('list', { name: 'Runs' });
}

async function open(label: RegExp) {
  await userEvent.click(within(await runList()).getByRole('button', { name: label }));
}

function afters(api: ReturnType<typeof fakeApi>, id: string) {
  return api.requests(outputRoute(id)).map((request) => {
    const query = new URL(request.url).searchParams;
    return `${query.get('after') ?? ''}/${query.get('limit') ?? ''}`;
  });
}

describe('RunsSection', () => {
  it('lists runs with their status on the dashboard', async () => {
    runsServer([
      run(),
      run({ id: TEST_ID, label: 'cargo test', status: 'failed', exitCode: 3, finishedAt: 4_000 }),
    ]);
    renderApp('/dashboard');
    const list = await runList();
    expect(within(list).getByText('cargo build')).toBeInTheDocument();
    expect(within(list).getByText('Running')).toBeInTheDocument();
    expect(within(list).getByText('Failed')).toBeInTheDocument();
    expect(within(list).getByText('exit 3')).toBeInTheDocument();
    expect(within(list).getByText('3 s')).toBeInTheDocument();
  });

  it('explains how to start a run when there is none', async () => {
    runsServer([]);
    renderApp('/dashboard');
    expect(await screen.findByText(/nullradix run start/)).toBeInTheDocument();
  });

  it('follows the output of the chosen run and stops once it finished', async () => {
    const server = runsServer([run()], {
      [BUILD_ID]: pages(
        { lines: [line(1, 'compiling'), line(2, 'warning: unused', 'stderr')], finished: false },
        { lines: [line(3, 'done')], finished: false },
        { lines: [], finished: true },
      ),
    });
    renderApp('/dashboard');
    await open(/cargo build/);
    const output = await screen.findByRole('region', { name: 'cargo build' });
    expect(await within(output).findByText('done')).toBeInTheDocument();
    expect(within(output).getByText('warning: unused')).toHaveClass('stderr');
    await waitFor(
      () => {
        expect(afters(server.api, BUILD_ID)).toHaveLength(3);
      },
      { timeout: OUTPUT_POLL_MS * 4 },
    );
    await act(() => new Promise((resolve) => setTimeout(resolve, OUTPUT_POLL_MS + 300)));
    const limit = String(OUTPUT_PAGE);
    expect(afters(server.api, BUILD_ID)).toEqual([`0/${limit}`, `2/${limit}`, `3/${limit}`]);
    expect(within(output).queryByRole('button', { name: 'Cancel run' })).not.toBeInTheDocument();
  });

  it('starts empty when switching to another run', async () => {
    runsServer([run(), run({ id: TEST_ID, label: 'cargo test' })], {
      [BUILD_ID]: pages({ lines: [line(1, 'build line')], finished: true }),
      [TEST_ID]: pages({ lines: [line(1, 'test line')], finished: true }),
    });
    renderApp('/dashboard');
    await open(/cargo build/);
    expect(await screen.findByText('build line')).toBeInTheDocument();
    await open(/cargo test/);
    const output = await screen.findByRole('region', { name: 'cargo test' });
    expect(await within(output).findByText('test line')).toBeInTheDocument();
    expect(screen.queryByText('build line')).not.toBeInTheDocument();
  });

  it('stops following a run the daemon no longer knows', async () => {
    const server = runsServer([run()], {
      [BUILD_ID]: () => problem(404, 'run-not-found', 'gone'),
    });
    renderApp('/dashboard');
    await open(/cargo build/);
    expect(await screen.findByText(GONE_MESSAGE)).toBeInTheDocument();
    await act(() => new Promise((resolve) => setTimeout(resolve, OUTPUT_POLL_MS + 300)));
    expect(server.api.requests(outputRoute(BUILD_ID))).toHaveLength(1);
  });

  it('closes on a second click or Close and hands focus back to the row', async () => {
    runsServer([run()], { [BUILD_ID]: pages() });
    renderApp('/dashboard');
    await open(/cargo build/);
    const output = await screen.findByRole('region', { name: 'cargo build' });
    await userEvent.click(within(output).getByRole('button', { name: 'Close' }));
    expect(screen.queryByRole('region', { name: 'cargo build' })).not.toBeInTheDocument();
    expect(within(await runList()).getByRole('button', { name: /cargo build/ })).toHaveFocus();

    await open(/cargo build/);
    expect(await screen.findByRole('region', { name: 'cargo build' })).toBeInTheDocument();
    await open(/cargo build/);
    expect(screen.queryByRole('region', { name: 'cargo build' })).not.toBeInTheDocument();
  });

  it('cancels an active run and explains a run that already ended', async () => {
    const server = runsServer([run()], { [BUILD_ID]: pages() });
    renderApp('/dashboard');
    await open(/cargo build/);
    const output = await screen.findByRole('region', { name: 'cargo build' });
    await userEvent.click(within(output).getByRole('button', { name: 'Cancel run' }));
    await waitFor(() => {
      expect(server.api.requests(`POST /api/v1/runs/${BUILD_ID}/cancel`)).toHaveLength(1);
    });

    server.api.set(`POST /api/v1/runs/${BUILD_ID}/cancel`, () =>
      problem(409, 'run-finished', 'over'),
    );
    await userEvent.click(within(output).getByRole('button', { name: 'Cancel run' }));
    expect(await within(output).findByText(FINISHED_MESSAGE)).toBeInTheDocument();
  });

  it('updates the list when the change stream reports a run change', async () => {
    const server = runsServer([run()]);
    renderApp('/dashboard');
    await runList();
    await waitFor(() => {
      expect(FakeEventSource.instances).not.toHaveLength(0);
    });
    server.update([run({ status: 'succeeded', exitCode: 0, finishedAt: 2_000, version: 3 })]);
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        { seq: 4, entity: 'run', entityId: BUILD_ID, op: 'update', at: 2 },
        '4',
      );
    });
    expect(await screen.findByText('Succeeded')).toBeInTheDocument();
  });
});
