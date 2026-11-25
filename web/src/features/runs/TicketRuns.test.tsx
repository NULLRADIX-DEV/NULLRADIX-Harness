import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { boardServer } from '../../test/boardServer';
import { json, type Handler } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import {
  MERGE_ID,
  SHOT_ID,
  WORK_ID,
  dispatchRun,
  runDetail,
  transcriptLine as line,
} from '../../test/runFixtures';
import { TRANSCRIPT_POLL_MS, type TranscriptLine } from './dispatchQueries';
import type { Dispatch, Run } from './queries';

const KEY = 'ARC-0001';

interface Page {
  lines: TranscriptLine[];
  lastSeq: number;
  finished: boolean;
}

function pages(...served: Page[]): Handler {
  const queue = [...served];
  return () => json(200, queue.shift() ?? { lines: [], lastSeq: 0, finished: false });
}

const FINISHED_PAGE: Page = {
  lines: [
    line(1, 'say', 'Reading the login code'),
    line(2, 'run', 'cargo test', { reference: 'c1', detail: 'cargo test --workspace' }),
    line(3, 'fail', 'exit 1', { reference: 'c1' }),
  ],
  lastSeq: 3,
  finished: true,
};

function serve(runs: Run[], transcripts: Record<string, Handler> = {}) {
  const server = boardServer();
  server.api.set('GET /api/v1/runs', () => json(200, { items: runs }));
  for (const [id, handler] of Object.entries(transcripts)) {
    server.api.set(`GET /api/v1/runs/${id}/transcript`, handler);
    server.api.set(`GET /api/v1/runs/${id}`, () => json(200, runDetail({ id })));
  }
  return server;
}

async function openRunsTab(name: RegExp | string = /^Runs/) {
  const backlog = await screen.findByRole('region', { name: 'Backlog' });
  await userEvent.click(await within(backlog).findByRole('button', { name: /ARC-0001/ }));
  const panel = await screen.findByRole('dialog', { name: `Ticket ${KEY}` });
  await userEvent.click(await within(panel).findByRole('tab', { name }));
  return within(await within(panel).findByRole('tabpanel', { name }));
}

function mergeRun(): Run {
  const dispatch: Dispatch | null | undefined = dispatchRun().dispatch;
  return dispatchRun({
    id: MERGE_ID,
    status: 'failed',
    dispatch: dispatch ? { ...dispatch, kind: 'merge' } : null,
    result: {
      abort: { category: 'max-turns', reason: 'Stopped after 40 turns' },
      usage: { costUsd: 0.2, inputTokens: 500, outputTokens: 100, models: [] },
    },
  });
}

describe('ticket Runs tab', () => {
  it('says so when the ticket has no run', async () => {
    serve([]);
    renderApp('/board');
    const runs = await openRunsTab('Runs');
    expect(await runs.findByText('No runs for this ticket yet.')).toBeInTheDocument();
  });

  it('counts the runs and lists them with badge, cost, tokens and abort reason', async () => {
    const server = serve([dispatchRun(), mergeRun()], {
      [WORK_ID]: pages(FINISHED_PAGE),
      [MERGE_ID]: pages(FINISHED_PAGE),
    });
    renderApp('/board');
    const runs = await openRunsTab(/^Runs \(2\)$/);
    const [work, merge] = await runs.findAllByRole('region', { name: /^Run / });
    if (!work || !merge) throw new Error('two runs expected');
    expect(within(work).getByText('Succeeded')).toBeInTheDocument();
    const head = within(work).getByRole('button', { name: /Work run of/ });
    expect(head).toHaveAttribute('aria-expanded', 'true');
    expect(within(head).getByText(/\$0\.43/)).toBeInTheDocument();
    expect(within(head).getByText(/15\.5k tokens/)).toBeInTheDocument();
    expect(within(head).getByText(/sonnet-5/)).toBeInTheDocument();
    expect(within(merge).getByText('Failed: Turn limit')).toBeInTheDocument();
    expect(within(merge).getByText('Merge session')).toBeInTheDocument();
    expect(within(merge).getByRole('button')).toHaveAttribute('aria-expanded', 'false');
    expect(within(merge).queryByText('Stopped after 40 turns')).not.toBeInTheDocument();
    await userEvent.click(within(merge).getByRole('button'));
    expect(await within(merge).findByText('Stopped after 40 turns')).toBeInTheDocument();
    expect(server.api.requests('GET /api/v1/runs')[0]?.url).toContain(`ticketKey=${KEY}`);
  });

  it('shows settings, prompt, transcript, denied tools, screenshot and usage of an open run', async () => {
    serve([dispatchRun()], { [WORK_ID]: pages(FINISHED_PAGE) });
    renderApp('/board');
    const runs = await openRunsTab(/^Runs/);
    const chips = await runs.findByRole('list', { name: 'Settings of the run' });
    expect(within(chips).getByText('Profile standard')).toBeInTheDocument();
    expect(within(chips).getByText('40 turns max')).toBeInTheDocument();
    expect(await runs.findByText('Fix the login of ARC-0001')).toBeInTheDocument();

    const transcript = await runs.findByRole('region', { name: 'Transcript' });
    expect(await within(transcript).findByText('Reading the login code')).toBeInTheDocument();
    expect(within(transcript).getByText('cargo test').closest('li')).toHaveClass('run-step-fail');
    await userEvent.click(within(transcript).getByRole('button', { name: 'Exact' }));
    expect(within(transcript).getByText('cargo test --workspace')).toBeInTheDocument();

    const denied = await runs.findByRole('region', { name: 'Denied tools' });
    expect(within(denied).getByText('Bash')).toBeInTheDocument();
    expect(within(denied).getByText(/2/)).toBeInTheDocument();

    const thumb = await runs.findByRole('button', { name: 'Screenshot 1 of 1' });
    expect(thumb.querySelector('img')).toHaveAttribute(
      'src',
      `/api/v1/runs/${WORK_ID}/screenshots/${SHOT_ID}`,
    );
    await userEvent.click(thumb);
    expect(runs.getByRole('img', { name: 'Screenshot 1' })).toBeInTheDocument();
    await userEvent.click(runs.getByRole('button', { name: 'Close screenshot' }));
    expect(runs.queryByRole('img', { name: 'Screenshot 1' })).not.toBeInTheDocument();

    const usage = runs.getByRole('region', { name: 'Usage' });
    expect(within(usage).getByText(/12\.4k in, 3\.1k out, 80k cache read/)).toBeInTheDocument();
  });

  it('lists every model when more than one was used', async () => {
    const run = dispatchRun();
    if (run.result?.usage) {
      run.result.usage.models = [
        { model: 'claude-sonnet-5', costUsd: 0.3, inputTokens: 100 },
        { model: 'claude-haiku-5', costUsd: 0.13, inputTokens: 50 },
      ];
    }
    serve([run], { [WORK_ID]: pages(FINISHED_PAGE) });
    renderApp('/board');
    const runs = await openRunsTab(/^Runs/);
    const usage = await runs.findByRole('region', { name: 'Usage' });
    expect(within(usage).getByText('haiku-5')).toBeInTheDocument();
    expect(within(usage).getByText(/50 in, \$0\.13/)).toBeInTheDocument();
  });

  it('polls the transcript of an active run until it finished', async () => {
    const running = dispatchRun({ status: 'running', finishedAt: null, result: null });
    const server = serve([running], {
      [WORK_ID]: pages(
        { lines: [line(1, 'say', 'first')], lastSeq: 1, finished: false },
        { lines: [line(2, 'say', 'second')], lastSeq: 2, finished: false },
        { lines: [], lastSeq: 2, finished: true },
      ),
    });
    renderApp('/board');
    const runs = await openRunsTab(/^Runs/);
    expect(
      await runs.findByText('second', {}, { timeout: TRANSCRIPT_POLL_MS * 3 }),
    ).toBeInTheDocument();
    expect(runs.getByText('first')).toBeInTheDocument();
    const route = `GET /api/v1/runs/${WORK_ID}/transcript`;
    await waitFor(
      () => {
        expect(server.api.requests(route)).toHaveLength(3);
      },
      { timeout: TRANSCRIPT_POLL_MS * 3 },
    );
    const afters = server.api
      .requests(route)
      .map((request) => new URL(request.url).searchParams.get('after'));
    expect(afters).toEqual(['0', '1', '2']);
  });
});
