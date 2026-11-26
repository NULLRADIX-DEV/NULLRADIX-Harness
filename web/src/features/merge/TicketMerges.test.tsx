import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { boardServer, entry, ticket } from '../../test/boardServer';
import { json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { canRetry } from './messages';
import type { MergeJob } from './queries';

const KEY = 'ARC-0001';
const BASE = `/api/v1/tickets/${KEY}`;
const BRANCH = 'ticket/ARC-0001-show-the-board';

function job(overrides: Partial<MergeJob> = {}): MergeJob {
  return {
    id: '0190a0b0-0000-7000-8000-0000000000b1',
    branch: BRANCH,
    phase: 'failed',
    targetBranch: 'main',
    strategy: 'merge',
    conflictSeen: false,
    problem: 'changes are staged',
    queuedAt: 1_000,
    version: 1,
    ...overrides,
  };
}

function serve(jobs: MergeJob[]) {
  const server = boardServer({
    entries: [
      entry(
        ticket({
          branch: BRANCH,
          status: 'blocked',
          column: 'blocked',
          blockedReason: 'Merge: changes are staged',
        }),
      ),
    ],
  });
  server.api.set(`GET ${BASE}/merges`, () => json(200, { items: jobs }));
  return server;
}

async function openMerges() {
  const blocked = await screen.findByRole('region', { name: 'Blocked' });
  await userEvent.click(await within(blocked).findByRole('button', { name: /ARC-0001/ }));
  const panel = await screen.findByRole('dialog', { name: `Ticket ${KEY}` });
  await userEvent.click(await within(panel).findByRole('tab', { name: 'Merges' }));
  return within(await within(panel).findByRole('tabpanel', { name: 'Merges' }));
}

describe('ticket Merges tab', () => {
  it('lists the merges with their problem and retries a blocked one with the ticket version', async () => {
    const server = serve([job()]);
    server.api.set(`POST ${BASE}/merge`, () =>
      json(200, { outcome: 'merged', text: `${KEY} merged into main (abc1234).`, ticket: {} }),
    );
    renderApp('/board');
    const merges = await openMerges();
    expect(await merges.findByText('changes are staged')).toBeInTheDocument();
    expect(merges.getByText('Failed')).toBeInTheDocument();
    await userEvent.click(merges.getByRole('button', { name: 'Retry merge' }));
    expect(await merges.findByText(`${KEY} merged into main (abc1234).`)).toBeInTheDocument();
    const sent = (await server.api.requests(`POST ${BASE}/merge`)[0]?.json()) as unknown;
    expect(sent).toEqual({ version: 1 });
    await waitFor(() => {
      expect(server.api.requests(`GET ${BASE}/merges`).length).toBeGreaterThan(1);
    });
  });

  it('says what went wrong when the retry is refused', async () => {
    const server = serve([job()]);
    server.api.set(`POST ${BASE}/merge`, () => problem(422, 'merge-not-possible', 'x'));
    renderApp('/board');
    const merges = await openMerges();
    await userEvent.click(await merges.findByRole('button', { name: 'Retry merge' }));
    expect(await merges.findByRole('alert')).toHaveTextContent(
      'This ticket waits for no merge any more.',
    );
  });

  it('offers a retry only for a done or merge-blocked ticket with a branch', () => {
    expect(canRetry('done', null, BRANCH)).toBe(true);
    expect(canRetry('blocked', 'Merge: conflict', BRANCH)).toBe(true);
    expect(canRetry('blocked', 'Human Interaction: look', BRANCH)).toBe(false);
    expect(canRetry('done', null, null)).toBe(false);
    expect(canRetry('planned', null, BRANCH)).toBe(false);
  });
});
