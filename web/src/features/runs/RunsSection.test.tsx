import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { FakeEventSource } from '../../test/eventSource';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { FINISHED_MESSAGE } from './RunsSection';
import type { OutputLine, Run } from './queries';

const BUILD_ID = '0190a0b0-0000-7000-8000-0000000000a1';
const TEST_ID = '0190a0b0-0000-7000-8000-0000000000a2';

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

// In-memory daemon for the runs routes; output pages are served in order.
function runsServer(runs: Run[], pages: { lines: OutputLine[]; finished: boolean }[] = []) {
  let items = runs;
  const outputs = [...pages];
  const api = fakeApi({
    'GET /api/v1/runs': () => json(200, { items }),
    [`GET /api/v1/runs/${BUILD_ID}/output`]: () =>
      json(200, outputs.shift() ?? { lines: [], finished: false }),
    [`POST /api/v1/runs/${BUILD_ID}/cancel`]: () => json(202, items[0]),
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
    expect(within(list).getByText(/exit 3/)).toBeInTheDocument();
    expect(within(list).getByText(/3 s/)).toBeInTheDocument();
  });

  it('explains how to start a run when there is none', async () => {
    runsServer([]);
    renderApp('/dashboard');
    expect(await screen.findByText(/nullradix run start/)).toBeInTheDocument();
  });

  it('follows the output of the chosen run until it finishes', async () => {
    const server = runsServer(
      [run()],
      [
        { lines: [line(1, 'compiling'), line(2, 'warning: unused', 'stderr')], finished: false },
        { lines: [line(3, 'done')], finished: false },
        { lines: [], finished: true },
      ],
    );
    renderApp('/dashboard');
    await userEvent.click(within(await runList()).getByRole('button', { name: /cargo build/ }));
    const output = await screen.findByRole('region', { name: 'cargo build' });
    expect(await within(output).findByText('done')).toBeInTheDocument();
    expect(within(output).getByText('warning: unused')).toHaveClass('stderr');
    await waitFor(() => {
      expect(server.api.requests(`GET /api/v1/runs/${BUILD_ID}/output`)).toHaveLength(3);
    });
    const afters = server.api
      .requests(`GET /api/v1/runs/${BUILD_ID}/output`)
      .map((request) => new URL(request.url).searchParams.get('after'));
    expect(afters).toEqual(['0', '2', '3']);
  });

  it('cancels an active run and explains a run that already ended', async () => {
    const server = runsServer([run()], [{ lines: [], finished: false }]);
    renderApp('/dashboard');
    await userEvent.click(within(await runList()).getByRole('button', { name: /cargo build/ }));
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
