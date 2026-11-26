import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { boardServer, entry, ticket } from '../../test/boardServer';
import { FakeEventSource } from '../../test/eventSource';
import { json, problem, type Handler } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { Review, ReviewFile } from './queries';
import { ARROW, DOT, lines } from './symbols';

const KEY = 'ARC-0001';
const BASE = `/api/v1/tickets/${KEY}`;
const BRANCH = 'ticket/ARC-0001-show-the-board';

function file(path: string, overrides: Partial<ReviewFile> = {}): ReviewFile {
  return { path, status: 'M', additions: 1, deletions: 1, binary: false, ...overrides };
}

function review(overrides: Partial<Review> = {}): Review {
  return {
    ticketKey: KEY,
    branch: BRANCH,
    target: 'main',
    base: '0123456789abcdef0123456789abcdef01234567',
    files: [
      file('src/price.ts'),
      file('src/discount.ts', { status: 'A', additions: 3, deletions: 0 }),
      file('src/old.ts', { status: 'D', additions: 0, deletions: 4 }),
      file('docs/hint.md', { status: 'R', oldPath: 'notes/note.md', additions: 0, deletions: 0 }),
      file('logo.bin', { status: 'A', additions: 0, deletions: 0, binary: true }),
    ],
    filesTruncated: false,
    tests: [
      { command: 'npx vitest run', ok: true, output: 'Tests 7 passed (7)' },
      { command: 'npm run lint', ok: false, output: '1 problem (1 error)' },
      { command: 'cargo test', ok: null, output: '' },
    ],
    testsAt: 1_000,
    runId: '0190a0b0-0000-7000-8000-0000000000a1',
    state: 'pending',
    available: true,
    reason: null,
    ...overrides,
  };
}

const PATCH =
  'diff --git a/src/price.ts b/src/price.ts\n--- a/src/price.ts\n+++ b/src/price.ts\n@@ -1 +1 @@\n-net\n+net * 0.9\n';

function serve(handlers: Record<string, Handler> = {}, branch: string | null = BRANCH) {
  const server = boardServer({
    entries: [entry(ticket({ branch, status: 'blocked', column: 'blocked' }))],
  });
  server.api.set(`GET ${BASE}/review`, () => json(200, review()));
  server.api.set(`GET ${BASE}/review/diff`, () =>
    json(200, { path: 'src/price.ts', patch: PATCH, truncated: false }),
  );
  for (const [route, handler] of Object.entries(handlers)) server.api.set(route, handler);
  return server;
}

async function openReview() {
  const blocked = await screen.findByRole('region', { name: 'Blocked' });
  await userEvent.click(await within(blocked).findByRole('button', { name: /ARC-0001/ }));
  const panel = await screen.findByRole('dialog', { name: `Ticket ${KEY}` });
  await userEvent.click(await within(panel).findByRole('tab', { name: 'Review' }));
  return within(await within(panel).findByRole('tabpanel', { name: 'Review' }));
}

describe('ticket Review tab', () => {
  it('shows the tab only for a ticket with a branch and loads nothing before it is opened', async () => {
    const server = serve({}, null);
    renderApp('/board');
    const blocked = await screen.findByRole('region', { name: 'Blocked' });
    await userEvent.click(await within(blocked).findByRole('button', { name: /ARC-0001/ }));
    const panel = await screen.findByRole('dialog', { name: `Ticket ${KEY}` });
    await within(panel).findByRole('tab', { name: 'Description' });
    expect(within(panel).queryByRole('tab', { name: 'Review' })).not.toBeInTheDocument();
    expect(server.api.requests(`GET ${BASE}/review`)).toHaveLength(0);
  });

  it('lists the changed files as a tree with their status and lines, and the tests', async () => {
    serve();
    renderApp('/board');
    const review = await openReview();
    expect(await review.findByText(BRANCH)).toBeInTheDocument();
    expect(review.getByText('main')).toBeInTheDocument();
    expect(review.getByText(/from 01234567/)).toBeInTheDocument();
    expect(
      review.getByText('The ticket waits for a review before it is merged.'),
    ).toBeInTheDocument();
    expect(review.getByText(`5 files changed, ${lines(4, 5)}`)).toBeInTheDocument();
    const src = review.getByText('src').closest('details');
    if (!src) throw new Error('the src folder is missing');
    expect(within(src).getByText(`3 files ${DOT} ${lines(4, 5)}`)).toBeInTheDocument();
    expect(within(src).getByText('new')).toBeInTheDocument();
    expect(within(src).getByText('deleted')).toBeInTheDocument();
    expect(review.getByText(`note.md ${ARROW} hint.md`)).toBeInTheDocument();
    expect(review.getByText('binary')).toBeInTheDocument();

    const tests = review.getByText('Tests the agent ran').parentElement;
    if (!tests) throw new Error('the tests are missing');
    expect(within(tests).getByText('green')).toBeInTheDocument();
    expect(within(tests).getByText('red')).toBeInTheDocument();
    expect(within(tests).getByText('no result')).toBeInTheDocument();
    expect(within(tests).getByText('1 problem (1 error)')).toBeInTheDocument();
    expect(within(tests).getByText('(no output)')).toBeInTheDocument();
  });

  it('loads the patch of a file when it is opened and colors its lines', async () => {
    const server = serve();
    renderApp('/board');
    const review = await openReview();
    expect(server.api.requests(`GET ${BASE}/review/diff`)).toHaveLength(0);
    await userEvent.click(await review.findByText('price.ts'));
    const patch = await review.findByLabelText('Changes of src/price.ts');
    expect(within(patch).getByText('+net * 0.9')).toHaveClass('add');
    expect(within(patch).getByText('-net')).toHaveClass('del');
    expect(within(patch).getByText('@@ -1 +1 @@')).toHaveClass('hunk');
    expect(within(patch).getByText('+++ b/src/price.ts')).toHaveClass('meta');
    const asked = server.api.requests(`GET ${BASE}/review/diff`);
    expect(asked).toHaveLength(1);
    expect(new URL(asked[0]?.url ?? '').searchParams.get('path')).toBe('src/price.ts');
  });

  it('shows a file at the head of the branch, and says so for a binary file', async () => {
    const server = serve({
      [`GET ${BASE}/review/files/src/price.ts`]: () =>
        json(200, {
          path: 'src/price.ts',
          size: 14,
          binary: false,
          text: 'export const x\n',
          truncated: false,
        }),
      [`GET ${BASE}/review/files/logo.bin`]: () =>
        json(200, { path: 'logo.bin', size: 5, binary: true, text: null, truncated: false }),
    });
    renderApp('/board');
    const review = await openReview();
    await userEvent.click(await review.findByText('price.ts'));
    await userEvent.click(await review.findByRole('button', { name: 'File' }));
    const text = await review.findByLabelText('Content of src/price.ts');
    expect(text).toHaveTextContent('export const x');
    expect(server.api.requests(`GET ${BASE}/review/files/src/price.ts`)).toHaveLength(1);
    await userEvent.click(review.getByRole('button', { name: 'Changes' }));
    expect(await review.findByLabelText('Changes of src/price.ts')).toBeInTheDocument();

    await userEvent.click(review.getByText('logo.bin'));
    const logo = within(review.getByText('logo.bin').closest('details') ?? document.body);
    await userEvent.click(await logo.findByRole('button', { name: 'File' }));
    expect(await logo.findByText('A binary file of 5 B.')).toBeInTheDocument();
  });

  it('cuts the notes of a large file and a large patch', async () => {
    serve({
      [`GET ${BASE}/review/diff`]: () =>
        json(200, { path: 'src/price.ts', patch: PATCH, truncated: true }),
      [`GET ${BASE}/review/files/src/price.ts`]: () =>
        json(200, {
          path: 'src/price.ts',
          size: 2048,
          binary: false,
          text: 'start',
          truncated: true,
        }),
    });
    renderApp('/board');
    const review = await openReview();
    await userEvent.click(await review.findByText('price.ts'));
    expect(await review.findByText('The patch is very large and was cut.')).toBeInTheDocument();
    await userEvent.click(review.getByRole('button', { name: 'File' }));
    expect(
      await review.findByText('The file has 2.0 KB; only its start is shown.'),
    ).toBeInTheDocument();
  });

  it('does not offer the file of a deleted path', async () => {
    serve();
    renderApp('/board');
    const review = await openReview();
    await userEvent.click(await review.findByText('old.ts'));
    const old = within(review.getByText('old.ts').closest('details') ?? document.body);
    expect(await old.findByRole('button', { name: 'File' })).toBeDisabled();
  });

  it('says why the changes cannot be shown and still lists the tests', async () => {
    serve({
      [`GET ${BASE}/review`]: () =>
        json(
          200,
          review({
            available: false,
            files: [],
            reason: 'the changes cannot be computed: git is broken',
          }),
        ),
    });
    renderApp('/board');
    const view = await openReview();
    expect(
      await view.findByText('the changes cannot be computed: git is broken'),
    ).toBeInTheDocument();
    expect(view.getByText('npx vitest run')).toBeInTheDocument();
    expect(view.queryByText(/files? changed/)).not.toBeInTheDocument();
  });

  it('notes a branch with more files than the list holds and one without changes', async () => {
    serve({
      [`GET ${BASE}/review`]: () => json(200, review({ filesTruncated: true })),
    });
    renderApp('/board');
    const view = await openReview();
    expect(
      await view.findByText('There are more changed files than are listed here.'),
    ).toBeInTheDocument();
  });

  it('shows the message of a failed load and an empty review', async () => {
    serve({ [`GET ${BASE}/review`]: () => problem(422, 'review-not-available', 'no') });
    renderApp('/board');
    const failed = await openReview();
    expect(
      await failed.findByText('There is nothing to review for this ticket.'),
    ).toBeInTheDocument();
  });

  it('says there are no changes against the target', async () => {
    serve({
      [`GET ${BASE}/review`]: () => json(200, review({ files: [], tests: [], state: null })),
    });
    renderApp('/board');
    const view = await openReview();
    expect(await view.findByText('No changes against the target.')).toBeInTheDocument();
    expect(view.queryByText('Tests the agent ran')).not.toBeInTheDocument();
  });

  it('reloads when the worktree or the run changes', async () => {
    const server = serve();
    renderApp('/board');
    const view = await openReview();
    await view.findByText(BRANCH);
    const before = server.api.requests(`GET ${BASE}/review`).length;
    server.api.set(`GET ${BASE}/review`, () => json(200, review({ target: 'release/1' })));
    act(() => {
      FakeEventSource.latest().emit('change', { seq: 1, entity: 'worktree' });
    });
    expect(await view.findByText('release/1')).toBeInTheDocument();
    await waitFor(() => {
      expect(server.api.requests(`GET ${BASE}/review`).length).toBeGreaterThan(before);
    });
  });
});
