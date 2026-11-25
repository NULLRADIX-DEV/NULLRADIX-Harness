import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { BOARD_ID, PROJECT_ID, USER_ID, boardServer } from '../../test/boardServer';
import { json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { noteTitle } from './NotesDialog';
import type { Note } from './queries';

const NOTES = `/api/v1/projects/${PROJECT_ID}/boards/${BOARD_ID}/notes`;

function note(number: number, overrides: Partial<Note> = {}): Note {
  return {
    number,
    id: `0190a0b0-0000-7000-8000-${String(700 + number).padStart(12, '0')}`,
    boardId: BOARD_ID,
    authorId: USER_ID,
    authorName: 'claude',
    title: `Finding ${String(number)}`,
    body: '',
    labels: [],
    seenCount: 1,
    seenIn: [],
    state: 'open',
    createdAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

// The board fake plus an in-memory daemon for the note routes.
function noteServer(initial: Note[]) {
  const notes = new Map(initial.map((item) => [item.number, item]));
  const server = boardServer();
  const { api } = server;
  api.set(`GET ${NOTES}`, (request) => {
    const state = new URL(request.url).searchParams.get('state') ?? 'open';
    const items = [...notes.values()].filter((item) => state === 'all' || item.state === state);
    return json(200, { items });
  });
  api.set(`POST ${NOTES}`, async (request) => {
    const body = (await request.json()) as { title: string; labels: string[]; force: boolean };
    const known = [...notes.values()].find(
      (item) => item.state !== 'dropped' && item.title.toLowerCase() === body.title.toLowerCase(),
    );
    if (known && !body.force) {
      const similar = [{ number: known.number, title: known.title, state: known.state }];
      const answer = { type: 'x', title: 'Conflict', status: 409, code: 'note-similar' };
      return new Response(JSON.stringify({ ...answer, detail: 'known', similar }), {
        status: 409,
        headers: { 'content-type': 'application/problem+json' },
      });
    }
    const created = note(notes.size + 1, { title: body.title, labels: body.labels });
    notes.set(created.number, created);
    return json(201, created);
  });
  for (const number of [1, 2, 3]) {
    api.set(`POST /api/v1/notes/${String(number)}/seen`, () => {
      const item = notes.get(number);
      if (!item) return problem(404, 'note-not-found', 'gone');
      const seen = { ...item, seenCount: item.seenCount + 1 };
      notes.set(number, seen);
      return json(200, seen);
    });
    api.set(`POST /api/v1/notes/${String(number)}/promote`, () => {
      const item = notes.get(number);
      if (item?.state !== 'open') return problem(409, 'note-not-open', 'not open');
      const promoted = { ...item, state: 'promoted' as const, ticketKey: 'ARC-0002' };
      notes.set(number, promoted);
      return json(200, { note: promoted, ticket: {} });
    });
    api.set(`POST /api/v1/notes/${String(number)}/drop`, async (request) => {
      const { reason } = (await request.json()) as { reason: string };
      const item = notes.get(number);
      if (!item) return problem(404, 'note-not-found', 'gone');
      const dropped = { ...item, state: 'dropped' as const, reason };
      notes.set(number, dropped);
      return json(200, dropped);
    });
  }
  return api;
}

async function openNotes() {
  renderApp('/board');
  await screen.findByText('Show the board');
  await userEvent.click(screen.getByRole('button', { name: 'Notes' }));
  return within(screen.getByRole('dialog', { name: 'Notes on Main' }));
}

describe('NotesDialog', () => {
  it('lists open notes and filters by state', async () => {
    noteServer([
      note(1, {
        title: 'Cache never expires',
        body: 'Seen in the board view.',
        source: 'ARC-0001',
        seenCount: 3,
        seenIn: ['ARC-0002'],
        labels: ['cache'],
      }),
      note(2, { title: 'Old idea', state: 'dropped', reason: 'not worth it' }),
      note(3, { title: 'Became work', state: 'promoted', ticketKey: 'ARC-0004' }),
    ]);
    const dialog = await openNotes();
    const list = within(dialog.getByRole('list', { name: 'Notes' }));
    expect(await list.findByText('Cache never expires')).toBeInTheDocument();
    expect(list.getByText('Seen in the board view.')).toBeInTheDocument();
    expect(list.getByText('seen 3 times')).toHaveAttribute('title', 'also at ARC-0002');
    expect(list.getByRole('button', { name: 'from ARC-0001' })).toBeInTheDocument();
    expect(list.getByText('cache')).toBeInTheDocument();
    expect(list.queryByText('Old idea')).not.toBeInTheDocument();

    await userEvent.click(dialog.getByRole('button', { name: 'Dropped' }));
    expect(await list.findByText('dropped: not worth it')).toBeInTheDocument();
    expect(dialog.getByRole('button', { name: 'Dropped' })).toHaveAttribute('aria-pressed', 'true');
    await userEvent.click(dialog.getByRole('button', { name: 'Promoted' }));
    expect(await list.findByRole('button', { name: 'ARC-0004' })).toBeInTheDocument();
    expect(list.queryByRole('button', { name: 'Make a ticket' })).not.toBeInTheDocument();
  });

  it('offers to count a similar note or to add it anyway', async () => {
    const api = noteServer([note(1, { title: 'Cache never expires' })]);
    const dialog = await openNotes();
    await dialog.findByText('Cache never expires');
    await userEvent.type(dialog.getByRole('textbox', { name: 'New note' }), 'cache never expires');
    await userEvent.type(dialog.getByRole('textbox', { name: 'Labels of the note' }), 'ui, web');
    await userEvent.click(dialog.getByRole('button', { name: 'Add note' }));

    const similar = within(await dialog.findByRole('alert'));
    expect(similar.getByText('This finding probably exists already:')).toBeInTheDocument();
    await userEvent.click(similar.getByRole('button', { name: 'Same finding: count it' }));
    expect(await dialog.findByText('seen 2 times')).toBeInTheDocument();
    expect(dialog.getByRole('textbox', { name: 'New note' })).toHaveValue('');
    expect(api.requests('POST /api/v1/notes/1/seen')).toHaveLength(1);

    await userEvent.type(dialog.getByRole('textbox', { name: 'New note' }), 'Cache never expires');
    await userEvent.keyboard('{Enter}');
    await userEvent.click(await dialog.findByRole('button', { name: 'Add anyway' }));
    expect(await dialog.findAllByText('Cache never expires')).toHaveLength(2);
    const requests = api.requests(`POST ${NOTES}`);
    expect(await requests.at(0)?.clone().json()).toEqual({
      title: 'cache never expires',
      body: '',
      labels: ['ui', 'web'],
      force: false,
    });
    expect(await requests.at(-1)?.clone().json()).toMatchObject({ force: true });
  });

  it('promotes with an edited title and drops only with a reason', async () => {
    const api = noteServer([
      note(1, { title: 'Search is slow' }),
      note(2, { title: 'Flaky test' }),
    ]);
    const dialog = await openNotes();
    const list = within(dialog.getByRole('list', { name: 'Notes' }));
    await list.findByText('Search is slow');
    const [first] = list.getAllByRole('listitem');
    if (!first) throw new Error('no note');
    await userEvent.click(within(first).getByRole('button', { name: 'Make a ticket' }));
    const title = within(first).getByRole('textbox', { name: 'Ticket title' });
    expect(title).toHaveValue('Search is slow');
    await userEvent.clear(title);
    await userEvent.type(title, 'Speed up search');
    await userEvent.click(within(first).getByRole('button', { name: 'Create ticket' }));
    await waitFor(() => {
      expect(list.queryByText('Search is slow')).not.toBeInTheDocument();
    });
    expect(await api.requests('POST /api/v1/notes/1/promote').at(0)?.clone().json()).toEqual({
      title: 'Speed up search',
    });

    const [second] = list.getAllByRole('listitem');
    if (!second) throw new Error('no note');
    await userEvent.click(within(second).getByRole('button', { name: 'Drop' }));
    await userEvent.click(within(second).getByRole('button', { name: 'Confirm drop' }));
    expect(dialog.getByText('Nothing is dropped without a reason.')).toBeInTheDocument();
    expect(api.requests('POST /api/v1/notes/2/drop')).toHaveLength(0);
    await userEvent.type(within(second).getByRole('textbox', { name: 'Reason' }), 'fixed');
    await userEvent.click(within(second).getByRole('button', { name: 'Confirm drop' }));
    expect(await dialog.findByText(/^No open notes/)).toBeInTheDocument();
    expect(await api.requests('POST /api/v1/notes/2/drop').at(0)?.clone().json()).toEqual({
      reason: 'fixed',
    });
  });

  it('asks before it discards input and opens linked tickets', async () => {
    noteServer([note(1, { title: 'Cache never expires', source: 'ARC-0001' })]);
    const dialog = await openNotes();
    await dialog.findByText('Cache never expires');
    await userEvent.type(dialog.getByRole('textbox', { name: 'New note' }), 'Half');
    await userEvent.click(dialog.getByRole('button', { name: 'from ARC-0001' }));
    const confirm = within(screen.getByRole('alertdialog', { name: 'Discard input' }));
    await userEvent.click(confirm.getByRole('button', { name: 'Keep editing' }));
    expect(dialog.getByRole('textbox', { name: 'New note' })).toHaveValue('Half');

    await userEvent.click(dialog.getByRole('button', { name: 'from ARC-0001' }));
    await userEvent.click(screen.getByRole('button', { name: 'Discard' }));
    expect(screen.queryByRole('dialog', { name: 'Notes on Main' })).not.toBeInTheDocument();
    expect(await screen.findByRole('dialog', { name: 'Ticket ARC-0001' })).toBeInTheDocument();
  });

  it('shows the first body line for notes without a title', () => {
    expect(noteTitle(note(4, { title: ' ', body: '\n first \nsecond' }))).toBe('first');
    expect(noteTitle(note(5, { title: '', body: '' }))).toBe('Note 5');
  });
});
