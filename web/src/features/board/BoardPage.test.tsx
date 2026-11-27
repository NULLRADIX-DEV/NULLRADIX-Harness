import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { BOARD_ID, board, boardServer, entry, ticket } from '../../test/boardServer';
import { FakeEventSource } from '../../test/eventSource';
import { statusDto } from '../../test/dispatcherFixture';
import { json } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { boardKeys } from '../boards/queries';
import { ticketKeys } from '../tickets/queries';
import { BOARD_STORAGE_KEY } from './boardChoice';
import { chooseBoardId } from './useBoardChoice';

const OTHER_BOARD = '0190a0b0-0000-7000-8000-0000000000b2';
const OTHER_PROJECT = '0190a0b0-0000-7000-8000-000000000002';

function column(name: string) {
  return screen.findByRole('region', { name });
}

function dialog() {
  return within(screen.getByRole('dialog', { name: /New ticket/ }));
}

describe('BoardPage', () => {
  it('shows the dispatcher indicator and follows its live changes', async () => {
    const server = boardServer();
    let state: 'idle' | 'paused' = 'idle';
    server.api.set('GET /api/v1/dispatcher', () => json(200, statusDto({ state })));
    renderApp('/board');

    expect(await screen.findByRole('button', { name: 'Dispatcher: Idle' })).toBeInTheDocument();
    state = 'paused';
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        { seq: 1, entity: 'dispatcher-state', entityId: 'x', op: 'update', at: 1 },
        '1',
      );
    });
    expect(await screen.findByRole('button', { name: 'Dispatcher: Paused' })).toBeInTheDocument();
  });

  it('follows a board chosen elsewhere, like the switcher of a theme shell', async () => {
    const other = board({
      id: OTHER_BOARD,
      projectId: OTHER_PROJECT,
      projectKey: 'OPS',
      name: 'Ops',
    });
    boardServer({ boards: [board(), other], entries: [entry(ticket())] });
    renderApp('/board');
    expect(await screen.findByRole('combobox', { name: 'Board' })).toHaveValue(BOARD_ID);
    act(() => {
      chooseBoardId(OTHER_BOARD);
    });
    await waitFor(() => {
      expect(screen.getByRole('combobox', { name: 'Board' })).toHaveValue(OTHER_BOARD);
    });
  });

  it('shows the eight columns with the cards of each', async () => {
    boardServer({
      entries: [
        entry(
          ticket({
            title: 'Show the board',
            priority: 'high',
            labels: ['web', 'ui'],
            assignee: 'ben',
            needsPlanning: true,
            stepsDone: 1,
            stepsTotal: 3,
          }),
        ),
        entry(
          ticket({
            number: 2,
            title: 'Open a ticket',
            column: 'waiting',
            waiting: true,
            dependsOn: ['ARC-0001'],
            dependsOpen: ['ARC-0001'],
          }),
        ),
        entry(
          ticket({ number: 3, title: 'Plan it', status: 'in-progress', column: 'in-progress' }),
        ),
      ],
    });
    renderApp('/board');

    const backlog = await column('Backlog');
    const regions = screen
      .getAllByRole('region')
      .map((region) => region.getAttribute('aria-label'));
    expect(regions).toEqual([
      'Backlog',
      'Waiting',
      'In planning',
      'Planned',
      'In progress',
      'Blocked',
      'Done',
      'Not needed',
    ]);
    expect(await within(backlog).findByText('Show the board')).toBeInTheDocument();
    expect(within(backlog).getByText('ARC-0001')).toBeInTheDocument();
    expect(within(backlog).getByText('High')).toBeInTheDocument();
    expect(within(backlog).getByText('@ben')).toBeInTheDocument();
    expect(within(backlog).getByText('Planning')).toBeInTheDocument();
    expect(within(backlog).getByText('web')).toBeInTheDocument();
    expect(within(backlog).getByText('1/3 steps')).toBeInTheDocument();
    expect(within(backlog).getByRole('progressbar', { name: 'Steps done' })).toHaveAttribute(
      'aria-valuenow',
      '1',
    );

    const waiting = await column('Waiting');
    expect(within(waiting).getByText('Open a ticket')).toBeInTheDocument();
    expect(within(waiting).getByText('waits on ARC-0001')).toBeInTheDocument();
    expect(within(await column('In progress')).getByText('Plan it')).toBeInTheDocument();
    expect(within(await column('Done')).getByText('No tickets')).toBeInTheDocument();
  });

  it('shows an empty state without boards', async () => {
    boardServer({ boards: [], entries: [] });
    renderApp('/board');
    expect(await screen.findByText('No board yet')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'New ticket' })).toBeDisabled();
  });

  it('remembers the chosen board', async () => {
    const other = board({
      id: OTHER_BOARD,
      projectId: OTHER_PROJECT,
      projectKey: 'NX',
      name: 'Nexus',
    });
    const { api } = boardServer({
      boards: [board(), other],
      entries: [
        entry(ticket()),
        entry(ticket({ number: 2, key: 'NX-0001', boardId: OTHER_BOARD, title: 'Elsewhere' })),
      ],
    });
    const view = renderApp('/board');
    const picker = await screen.findByRole('combobox', { name: 'Board' });
    expect(within(picker).getByRole('group', { name: 'NX' })).toBeInTheDocument();
    expect(await screen.findByText('Show the board')).toBeInTheDocument();

    await userEvent.selectOptions(picker, 'Nexus');
    expect(await screen.findByText('Elsewhere')).toBeInTheDocument();
    expect(screen.queryByText('Show the board')).not.toBeInTheDocument();
    expect(window.localStorage.getItem(BOARD_STORAGE_KEY)).toBe(OTHER_BOARD);
    const lists = api.requests('GET /api/v1/tickets');
    expect(new URL(lists.at(-1)?.url ?? '').searchParams.get('boardId')).toBe(OTHER_BOARD);

    view.unmount();
    renderApp('/board');
    expect(await screen.findByText('Elsewhere')).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Board' })).toHaveValue(OTHER_BOARD);
  });

  it('opens the board and the notes dialog an inbox link names, then clears the address', async () => {
    const other = board({
      id: OTHER_BOARD,
      projectId: OTHER_PROJECT,
      projectKey: 'NX',
      name: 'Nexus',
    });
    boardServer({ boards: [board(), other], entries: [entry(ticket())] });
    renderApp(`/board?board=${OTHER_BOARD}&notes=1`);
    expect(await screen.findByRole('dialog', { name: 'Notes on Nexus' })).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Board' })).toHaveValue(OTHER_BOARD);
    expect(window.localStorage.getItem(BOARD_STORAGE_KEY)).toBe(OTHER_BOARD);
    await userEvent.click(screen.getByRole('button', { name: 'Close notes' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('creates a ticket with the fields of the dialog', async () => {
    const { api } = boardServer();
    renderApp('/board');
    await screen.findByText('Show the board');
    await userEvent.click(screen.getByRole('button', { name: 'New ticket' }));

    const form = dialog();
    expect(form.getByRole('textbox', { name: 'Title' })).toHaveFocus();
    await userEvent.type(form.getByRole('textbox', { name: 'Title' }), 'Add tickets');
    await userEvent.type(form.getByRole('textbox', { name: 'Description' }), 'From the board');
    await userEvent.selectOptions(form.getByRole('combobox', { name: 'Priority' }), 'Urgent');
    await userEvent.type(form.getByRole('textbox', { name: 'Labels' }), 'web, ui');
    await userEvent.type(form.getByRole('textbox', { name: 'Depends on' }), 'ARC-0001');
    await userEvent.type(form.getByRole('textbox', { name: 'Acceptance' }), 'It shows up');
    await userEvent.click(form.getByRole('checkbox', { name: 'Needs planning' }));
    await userEvent.click(form.getByRole('button', { name: 'Create' }));

    expect(await screen.findByText('ARC-0002 created.')).toHaveAttribute('role', 'status');
    expect(screen.queryByRole('dialog', { name: /New ticket/ })).not.toBeInTheDocument();
    expect(await within(await column('Backlog')).findByText('Add tickets')).toBeInTheDocument();
    const [request] = api.requests(
      `POST /api/v1/projects/${board().projectId}/boards/${BOARD_ID}/tickets`,
    );
    expect(await request?.clone().json()).toEqual({
      title: 'Add tickets',
      description: 'From the board',
      priority: 'urgent',
      labels: ['web', 'ui'],
      dependsOn: ['ARC-0001'],
      acceptance: 'It shows up',
      needsPlanning: false,
    });
  });

  it('names the existing ticket of a duplicate and creates it anyway on request', async () => {
    const { api } = boardServer();
    renderApp('/board');
    await screen.findByText('Show the board');
    await userEvent.click(screen.getByRole('button', { name: 'New ticket' }));
    const form = dialog();
    await userEvent.type(form.getByRole('textbox', { name: 'Title' }), 'Show the board');
    await userEvent.click(form.getByRole('button', { name: 'Create' }));

    expect(await form.findByRole('alert')).toHaveTextContent(
      'An open ticket with this title already exists: ARC-0001.',
    );
    await userEvent.click(form.getByRole('button', { name: 'Create anyway' }));
    expect(await screen.findByText('ARC-0002 created.')).toHaveAttribute('role', 'status');
    const requests = api.requests(
      `POST /api/v1/projects/${board().projectId}/boards/${BOARD_ID}/tickets`,
    );
    expect(await requests.at(-1)?.clone().json()).toMatchObject({ allowDuplicate: true });
  });

  it('asks before it discards the input of a new ticket', async () => {
    const { api } = boardServer();
    renderApp('/board');
    await screen.findByText('Show the board');
    await userEvent.click(screen.getByRole('button', { name: 'New ticket' }));
    await userEvent.type(dialog().getByRole('textbox', { name: 'Title' }), 'Half done');
    await userEvent.keyboard('{Escape}');

    const confirm = within(screen.getByRole('alertdialog', { name: 'Discard input' }));
    await userEvent.click(confirm.getByRole('button', { name: 'Keep editing' }));
    expect(dialog().getByRole('textbox', { name: 'Title' })).toHaveValue('Half done');
    await userEvent.click(dialog().getByRole('button', { name: 'Cancel' }));
    await userEvent.click(screen.getByRole('button', { name: 'Discard' }));
    expect(screen.queryByRole('dialog', { name: /New ticket/ })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'New ticket' })).toHaveFocus();
    expect(
      api.requests(`POST /api/v1/projects/${board().projectId}/boards/${BOARD_ID}/tickets`),
    ).toHaveLength(0);
  });

  it('refetches the board when the change stream reports a ticket change', async () => {
    const server = boardServer();
    renderApp('/board');
    await screen.findByText('Show the board');
    await waitFor(() => {
      expect(FakeEventSource.instances).not.toHaveLength(0);
    });
    server.bumpElsewhere('ARC-0001', { title: 'Show the board live' });
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        { seq: 4, entity: 'ticket', entityId: 'x', op: 'update', at: 2 },
        '4',
      );
    });
    expect(await screen.findByText('Show the board live')).toBeInTheDocument();
    expect(ticketKeys.list(BOARD_ID)).toEqual(['tickets', 'list', BOARD_ID]);
    expect(boardKeys.list()).toEqual(['boards', 'list']);
  });
});
