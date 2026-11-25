import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { BOARD_ID, board } from '../../test/boardServer';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { moveItem, type InboxItem, type InboxList } from './queries';

const INBOX = '/api/v1/me/inbox';
const ORDER = '/api/v1/me/inbox/order';

function entry(overrides: Partial<InboxItem> = {}): InboxItem {
  return {
    type: 'ticket',
    key: 'ARC-0001',
    kind: 'merge',
    title: 'Merge worktrees',
    detail: 'Merge: conflict in a.rs',
    boardId: BOARD_ID,
    boardName: 'Main',
    projectKey: 'ARC',
    waitingTickets: 0,
    priority: 'normal',
    sinceAt: 1,
    rank: 1,
    ...overrides,
  };
}

const ENTRIES: InboxItem[] = [
  entry(),
  entry({
    type: 'question',
    key: 'ARC-Q0001',
    kind: 'question',
    title: 'Which store?',
    detail: 'planning ARC-0004',
    waitingTickets: 1,
    priority: null,
    rank: 3,
  }),
  entry({
    key: 'ARC-0002',
    kind: 'accept',
    title: 'Login page',
    detail: 'Human Interaction: look at it',
    rank: 4,
  }),
  entry({
    type: 'note',
    key: '1',
    kind: 'note',
    title: 'Login page is slow',
    detail: null,
    priority: null,
    rank: 7,
  }),
];

function list(items: InboxItem[] = ENTRIES): InboxList {
  return {
    items,
    counts: { merge: 1, setup: 0, question: 1, accept: 1, blocked: 0, planning: 0, note: 1 },
    total: items.length,
  };
}

function server(found: InboxList = list()) {
  return fakeApi({
    'GET /api/v1/boards': () => json(200, { items: [board()] }),
    [`GET ${INBOX}`]: () => json(200, found),
    [`PUT ${ORDER}`]: async (request) => json(200, await request.json()),
    [`DELETE ${ORDER}`]: () => new Response(null, { status: 204 }),
  });
}

function rowAt(index: number): HTMLElement {
  const rows = within(screen.getByRole('list', { name: 'Entries' })).getAllByRole('listitem');
  const found = rows[index];
  if (!found) throw new Error(`no row ${String(index)}`);
  return found;
}

function titles(): string[] {
  const rows = within(screen.getByRole('list', { name: 'Entries' })).getAllByRole('listitem');
  return rows.map((row) => row.querySelector('.inbox-title')?.textContent ?? '');
}

describe('InboxPage', () => {
  it('lists entries with kinds, counts, places and links', async () => {
    server();
    renderApp('/inbox');
    expect(await screen.findByText('Which store?')).toBeInTheDocument();
    expect(titles()).toEqual([
      'ARC-0001 Merge worktrees',
      'ARC-Q0001 Which store?',
      'ARC-0002 Login page',
      'Note 1 Login page is slow',
    ]);
    expect(screen.getByRole('button', { name: 'Merge failed (1)' })).toBeInTheDocument();
    expect(screen.getByText(/1 ticket waits/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'ARC-Q0001 Which store?' })).toHaveAttribute(
      'href',
      '/questions?question=ARC-Q0001',
    );
    expect(screen.getByRole('link', { name: 'ARC-0001 Merge worktrees' })).toHaveAttribute(
      'href',
      '/board?ticket=ARC-0001',
    );
    expect(screen.getByRole('button', { name: 'Accept' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Make ticket' })).toBeInTheDocument();
  });

  it('filters by kind and board through the address', async () => {
    const api = server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    await userEvent.click(screen.getByRole('button', { name: 'Note (1)' }));
    expect(titles()).toEqual(['Note 1 Login page is slow']);
    await userEvent.click(screen.getByRole('button', { name: 'Note (1)' }));
    expect(titles()).toHaveLength(4);
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Board' }), BOARD_ID);
    await screen.findByText('Reordering works only without filters.');
    const asked = api.requests(`GET ${INBOX}`).map((request) => new URL(request.url));
    expect(asked.at(-1)?.searchParams.get('boardId')).toBe(BOARD_ID);
  });

  it('saves the order after a drop and resets it', async () => {
    const api = server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    fireEvent.dragStart(rowAt(3), { dataTransfer: { setData: () => undefined } });
    fireEvent.dragOver(rowAt(0));
    fireEvent.drop(rowAt(0));
    await waitFor(() => {
      expect(titles()[0]).toBe('Note 1 Login page is slow');
    });
    const saved = api.requests(`PUT ${ORDER}`);
    expect(saved).toHaveLength(1);
    expect(await saved[0]?.json()).toEqual({
      items: [
        { type: 'note', key: '1' },
        { type: 'ticket', key: 'ARC-0001' },
        { type: 'question', key: 'ARC-Q0001' },
        { type: 'ticket', key: 'ARC-0002' },
      ],
    });
    await userEvent.click(screen.getByRole('button', { name: 'Reset order' }));
    expect(api.requests(`DELETE ${ORDER}`)).toHaveLength(1);
  });

  it('restores the order when saving fails', async () => {
    const api = server();
    api.set(`PUT ${ORDER}`, () => problem(422, 'inbox-key-invalid', 'bad key'));
    renderApp('/inbox');
    await screen.findByText('Which store?');
    fireEvent.dragStart(rowAt(1), { dataTransfer: { setData: () => undefined } });
    fireEvent.drop(rowAt(0));
    expect(await screen.findByText('Could not save the order: bad key')).toBeInTheDocument();
    expect(titles()[0]).toBe('ARC-0001 Merge worktrees');
  });

  it('accepts a ticket and promotes a note from their rows', async () => {
    const api = server();
    api.set('POST /api/v1/tickets/ARC-0002/accept', () => json(200, {}));
    api.set('POST /api/v1/notes/1/promote', () => json(200, {}));
    renderApp('/inbox');
    await screen.findByText('Which store?');
    await userEvent.click(screen.getByRole('button', { name: 'Accept' }));
    await userEvent.click(screen.getByRole('button', { name: 'Make ticket' }));
    expect(api.requests('POST /api/v1/tickets/ARC-0002/accept')).toHaveLength(1);
    expect(api.requests('POST /api/v1/notes/1/promote')).toHaveLength(1);
    expect(api.requests(`GET ${INBOX}`).length).toBeGreaterThan(1);
  });

  it('says when nothing waits', async () => {
    server(list([]));
    renderApp('/inbox');
    expect(await screen.findByText('Nothing waits for you')).toBeInTheDocument();
  });
});

describe('moveItem', () => {
  it('moves one entry and keeps the list when nothing moves', () => {
    const items = ['a', 'b', 'c'];
    expect(moveItem(items, 2, 0)).toEqual(['c', 'a', 'b']);
    expect(moveItem(items, 0, 0)).toBe(items);
    expect(moveItem(items, 0, 5)).toBe(items);
  });
});
