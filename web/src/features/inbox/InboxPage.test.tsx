import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { BOARD_ID, PROJECT_ID, boardServer } from '../../test/boardServer';
import { FakeEventSource } from '../../test/eventSource';
import { json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { entryKey, moveItem, type InboxItem, type InboxList } from './queries';

const INBOX = '/api/v1/me/inbox';
const ORDER = '/api/v1/me/inbox/order';
const NOTES = `/api/v1/projects/${PROJECT_ID}/boards/${BOARD_ID}/notes`;
const NO_DATA = { dataTransfer: { setData: () => undefined } };

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

function list(items: InboxItem[], orderVersion: number): InboxList {
  return {
    items,
    counts: { merge: 1, setup: 0, question: 1, accept: 1, blocked: 0, planning: 0, note: 1 },
    total: items.length,
    orderVersion,
  };
}

interface OrderBody {
  items: { type: InboxItem['type']; key: string }[];
  version: number;
}

// The board fake plus an in-memory inbox that guards its saved order by a version.
function server(initial: InboxItem[] = ENTRIES) {
  const model = { items: [...initial], version: 1 };
  const { api } = boardServer();
  api.set(`GET ${INBOX}`, () => json(200, list(model.items, model.version)));
  api.set(`GET ${NOTES}`, () => json(200, { items: [] }));
  api.set(`PUT ${ORDER}`, async (request) => {
    const body = (await request.json()) as OrderBody;
    if (body.version !== model.version) return problem(409, 'version-conflict', 'stale');
    model.items = body.items.flatMap((ref) =>
      model.items.filter((item) => item.type === ref.type && item.key === ref.key),
    );
    model.version += 1;
    return json(200, { items: body.items, version: model.version });
  });
  api.set(`DELETE ${ORDER}`, (request) => {
    const version = new URL(request.url).searchParams.get('version');
    if (version !== String(model.version)) return problem(409, 'version-conflict', 'stale');
    model.items = [...initial];
    model.version += 1;
    return new Response(null, { status: 204 });
  });
  return { api, model };
}

function rows(): HTMLElement[] {
  return within(screen.getByRole('list', { name: 'Entries' })).getAllByRole('listitem');
}

function rowAt(index: number): HTMLElement {
  const found = rows()[index];
  if (!found) throw new Error(`no row ${String(index)}`);
  return found;
}

function titles(): string[] {
  return rows().map((row) => row.querySelector('.inbox-title')?.textContent ?? '');
}

async function sent(request: Request | undefined): Promise<OrderBody> {
  if (!request) throw new Error('nothing was sent');
  return (await request.json()) as OrderBody;
}

function announce(entity: string, seq: number) {
  act(() => {
    FakeEventSource.latest().emit(
      'change',
      { seq, entity, entityId: 'x', op: 'update', at: 2 },
      String(seq),
    );
  });
}

async function streamOpen() {
  await waitFor(() => {
    expect(FakeEventSource.instances).not.toHaveLength(0);
  });
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
    expect(screen.getByRole('link', { name: 'Note 1 Login page is slow' })).toHaveAttribute(
      'href',
      `/board?board=${BOARD_ID}&notes=1`,
    );
    expect(screen.getByRole('button', { name: 'Accept' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Make ticket' })).toBeInTheDocument();
  });

  it('opens the board of a note with its notes dialog', async () => {
    server();
    renderApp('/inbox');
    await userEvent.click(await screen.findByRole('link', { name: 'Note 1 Login page is slow' }));
    expect(await screen.findByRole('dialog', { name: 'Notes on Main' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Board' })).toBeInTheDocument();
  });

  it('filters by kind and board through the address', async () => {
    const { api } = server();
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
    const { api } = server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    fireEvent.dragStart(rowAt(3), NO_DATA);
    fireEvent.dragOver(rowAt(0));
    fireEvent.drop(rowAt(0));
    await waitFor(() => {
      expect(titles()[0]).toBe('Note 1 Login page is slow');
    });
    const saved = api.requests(`PUT ${ORDER}`);
    expect(saved).toHaveLength(1);
    expect(await sent(saved[0])).toEqual({
      items: [
        { type: 'note', key: '1' },
        { type: 'ticket', key: 'ARC-0001' },
        { type: 'question', key: 'ARC-Q0001' },
        { type: 'ticket', key: 'ARC-0002' },
      ],
      version: 1,
    });
    await waitFor(() => {
      expect(api.requests(`GET ${INBOX}`).length).toBeGreaterThan(1);
    });
    await userEvent.click(screen.getByRole('button', { name: 'Reset order' }));
    await waitFor(() => {
      expect(titles()[0]).toBe('ARC-0001 Merge worktrees');
    });
    const resets = api.requests(`DELETE ${ORDER}`);
    expect(resets).toHaveLength(1);
    expect(new URL(resets[0]?.url ?? '').searchParams.get('version')).toBe('2');
  });

  it('drags the entry it started with even when the list changed meanwhile', async () => {
    const { api, model } = server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    await streamOpen();
    fireEvent.dragStart(rowAt(3), NO_DATA);
    model.items = [entry({ key: 'ARC-0009', title: 'Fresh failure' }), ...ENTRIES];
    announce('ticket', 4);
    await screen.findByText('Fresh failure');
    fireEvent.drop(rowAt(1));
    await waitFor(() => {
      expect(api.requests(`PUT ${ORDER}`)).toHaveLength(1);
    });
    expect((await sent(api.requests(`PUT ${ORDER}`)[0])).items.map(entryKey)).toEqual([
      'ticket:ARC-0009',
      'note:1',
      'ticket:ARC-0001',
      'question:ARC-Q0001',
      'ticket:ARC-0002',
    ]);
  });

  it('forgets the dragged entry when the drag ends without a drop', async () => {
    const { api } = server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    fireEvent.dragStart(rowAt(3), NO_DATA);
    fireEvent.dragEnd(rowAt(3));
    fireEvent.drop(rowAt(0));
    expect(api.requests(`PUT ${ORDER}`)).toHaveLength(0);
    expect(titles()[0]).toBe('ARC-0001 Merge worktrees');
  });

  it('restores the order when saving fails', async () => {
    const { api } = server();
    api.set(`PUT ${ORDER}`, () => problem(422, 'inbox-key-invalid', 'bad key'));
    renderApp('/inbox');
    await screen.findByText('Which store?');
    fireEvent.dragStart(rowAt(1), NO_DATA);
    fireEvent.drop(rowAt(0));
    expect(await screen.findByText('Could not save the order: bad key')).toBeInTheDocument();
    expect(titles()[0]).toBe('ARC-0001 Merge worktrees');
  });

  it('says so and reloads when the order was saved elsewhere meanwhile', async () => {
    const { api, model } = server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    model.items = [ENTRIES[3] ?? entry(), ...ENTRIES.slice(0, 3)];
    model.version = 5;
    fireEvent.dragStart(rowAt(1), NO_DATA);
    fireEvent.drop(rowAt(0));
    expect(
      await screen.findByText(/The order was changed elsewhere\. The inbox is up to date now/),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(titles()[0]).toBe('Note 1 Login page is slow');
    });
    expect(api.requests(`GET ${INBOX}`).length).toBeGreaterThan(1);
  });

  it('keeps the newest order version when a slow reload brings an older one', async () => {
    const { api, model } = server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    await streamOpen();
    fireEvent.dragStart(rowAt(3), NO_DATA);
    fireEvent.drop(rowAt(0));
    await waitFor(() => {
      expect(model.version).toBe(2);
    });
    // a reload that read the database before the save answers with the old version
    api.set(`GET ${INBOX}`, () => json(200, list(model.items, 1)));
    announce('ticket', 8);
    await waitFor(() => {
      expect(api.requests(`GET ${INBOX}`).length).toBeGreaterThan(2);
    });
    fireEvent.dragStart(rowAt(2), NO_DATA);
    fireEvent.drop(rowAt(0));
    await waitFor(() => {
      expect(api.requests(`PUT ${ORDER}`)).toHaveLength(2);
    });
    expect((await sent(api.requests(`PUT ${ORDER}`)[1])).version).toBe(2);
    expect(screen.queryByText(/The order was changed elsewhere/)).not.toBeInTheDocument();
  });

  it('refetches when the change stream reports the saved order', async () => {
    const { api, model } = server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    await streamOpen();
    const before = api.requests(`GET ${INBOX}`).length;
    model.items = [ENTRIES[3] ?? entry(), ...ENTRIES.slice(0, 3)];
    model.version = 2;
    announce('inbox-order', 7);
    await waitFor(() => {
      expect(titles()[0]).toBe('Note 1 Login page is slow');
    });
    expect(api.requests(`GET ${INBOX}`).length).toBeGreaterThan(before);
  });

  it('moves an entry with the buttons and announces its place', async () => {
    const { api } = server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    expect(screen.getByRole('button', { name: 'Move ARC-0001 up' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Move Note 1 down' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Move ARC-0001 down' })).toBeEnabled();

    await userEvent.click(screen.getByRole('button', { name: 'Move ARC-0001 down' }));
    await waitFor(() => {
      expect(titles()[1]).toBe('ARC-0001 Merge worktrees');
    });
    expect(screen.getByRole('status', { name: 'Order changes' })).toHaveTextContent(
      'ARC-0001 moved to position 2 of 4.',
    );
    expect((await sent(api.requests(`PUT ${ORDER}`)[0])).items.map(entryKey)).toEqual([
      'question:ARC-Q0001',
      'ticket:ARC-0001',
      'ticket:ARC-0002',
      'note:1',
    ]);
    expect(screen.getByRole('button', { name: 'Move ARC-0001 down' })).toHaveFocus();
    expect(screen.getByRole('button', { name: 'Move ARC-Q0001 up' })).toBeDisabled();

    await userEvent.click(screen.getByRole('button', { name: 'Move ARC-0001 up' }));
    await waitFor(() => {
      expect(screen.getByRole('status', { name: 'Order changes' })).toHaveTextContent(
        'ARC-0001 moved to position 1 of 4.',
      );
    });
    await waitFor(() => {
      expect(api.requests(`PUT ${ORDER}`)).toHaveLength(2);
    });
    expect((await sent(api.requests(`PUT ${ORDER}`)[1])).version).toBe(2);
  });

  it('turns the move buttons off while a filter is on', async () => {
    server();
    renderApp('/inbox');
    await screen.findByText('Which store?');
    await userEvent.click(screen.getByRole('button', { name: 'Merge failed (1)' }));
    await userEvent.click(screen.getByRole('button', { name: 'Acceptance (1)' }));
    expect(screen.getByRole('button', { name: 'Move ARC-0001 down' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Move ARC-0002 up' })).toBeDisabled();
    expect(rowAt(0)).not.toHaveAttribute('draggable');
  });

  it('accepts a ticket and promotes a note from their rows', async () => {
    const { api } = server();
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
    server([]);
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
