import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { USER_ID, boardServer, entry, ticket } from '../../test/boardServer';
import { problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { HistoryLine, Step } from '../tickets/queries';
import { CONFLICT_MESSAGE, STEP_CONFLICT_MESSAGE } from './messages';

const KEY = 'ARC-0001';
const BASE = `/api/v1/tickets/${KEY}`;

const STEPS: Step[] = [
  { number: 1, text: 'Write the queries', done: true },
  { number: 2, text: 'Write the page', done: false },
];

const HISTORY: HistoryLine[] = [
  {
    at: Date.now(),
    actorId: USER_ID,
    actorName: 'Ben',
    field: 'status',
    from: null,
    to: 'backlog',
    note: 'created',
  },
  {
    at: Date.now(),
    actorId: USER_ID,
    actorName: 'Ben',
    field: 'blockedReason',
    from: null,
    to: 'Waiting for the API',
  },
];

function server() {
  return boardServer({
    entries: [
      entry(
        ticket({
          description: 'Columns and cards',
          labels: ['web'],
          stepsDone: 1,
          stepsTotal: 2,
          dependsOn: ['ARC-0002'],
          dependsOpen: ['ARC-0002'],
        }),
        STEPS,
        HISTORY,
      ),
      entry(ticket({ number: 2, title: 'Serve the API', dependents: [KEY] })),
    ],
  });
}

async function openTicket() {
  const backlog = await screen.findByRole('region', { name: 'Backlog' });
  await userEvent.click(await within(backlog).findByRole('button', { name: /ARC-0001/ }));
  const panel = await screen.findByRole('dialog', { name: `Ticket ${KEY}` });
  await within(panel).findByRole('textbox', { name: 'Title' });
  return within(panel);
}

async function lastBody(requests: Request[]): Promise<unknown> {
  return requests.at(-1)?.clone().json();
}

describe('TicketPanel', () => {
  it('opens a ticket from its card with fields, tabs and history', async () => {
    server();
    renderApp('/board');
    const panel = await openTicket();

    expect(panel.getByRole('textbox', { name: 'Title' })).toHaveValue('Show the board');
    expect(panel.getByText(/Version 1/)).toBeInTheDocument();
    expect(panel.getByRole('combobox', { name: 'Status' })).toHaveValue('backlog');
    expect(panel.getByRole('textbox', { name: 'Labels' })).toHaveValue('web');
    expect(panel.getByRole('tab', { name: 'Description' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(panel.getByText('Columns and cards')).toBeInTheDocument();
    expect(panel.getByRole('button', { name: 'ARC-0002' })).toHaveClass('is-open');

    await userEvent.click(panel.getByRole('tab', { name: 'History' }));
    const history = panel.getByRole('tabpanel', { name: 'History' });
    expect(within(history).getByText('Created in Backlog')).toBeInTheDocument();
    expect(within(history).getByText('Waiting for the API')).toBeInTheDocument();

    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: `Ticket ${KEY}` })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /ARC-0001/ })).toHaveFocus();
  });

  it('saves an edited title with the version it read', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openTicket();
    const title = panel.getByRole('textbox', { name: 'Title' });
    await userEvent.clear(title);
    await userEvent.type(title, 'Show the whole board{Enter}');

    expect(await panel.findByText(/Version 2/)).toBeInTheDocument();
    expect(await lastBody(api.requests(`PATCH ${BASE}`))).toEqual({
      title: 'Show the whole board',
      version: 1,
    });
    const backlog = screen.getByRole('region', { name: 'Backlog' });
    expect(await within(backlog).findByText('Show the whole board')).toBeInTheDocument();
  });

  it('saves the labels when the field is left', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openTicket();
    const labels = panel.getByRole('textbox', { name: 'Labels' });
    await userEvent.type(labels, ', ui ,');
    await userEvent.tab();

    await panel.findByText(/Version 2/);
    expect(await lastBody(api.requests(`PATCH ${BASE}`))).toEqual({
      labels: ['web', 'ui'],
      version: 1,
    });
    expect(labels).toHaveValue('web, ui');
  });

  it('reports a version conflict, refetches and saves on retry', async () => {
    const fake = server();
    const { api } = fake;
    renderApp('/board');
    const panel = await openTicket();
    fake.bumpElsewhere(KEY, { description: 'Changed elsewhere' });
    const reads = () => api.requests(`GET ${BASE}`).length;
    const before = reads();

    const title = panel.getByRole('textbox', { name: 'Title' });
    await userEvent.clear(title);
    await userEvent.type(title, 'Mine{Enter}');

    expect(await panel.findByRole('alert')).toHaveTextContent(CONFLICT_MESSAGE);
    await waitFor(() => {
      expect(reads()).toBeGreaterThan(before);
    });
    expect(await panel.findByText('Changed elsewhere')).toBeInTheDocument();
    expect(title).toHaveValue('Mine');

    await userEvent.type(title, '{Enter}');
    expect(await panel.findByText(/Version 3/)).toBeInTheDocument();
    expect(panel.queryByRole('alert')).not.toBeInTheDocument();
    expect(await lastBody(api.requests(`PATCH ${BASE}`))).toEqual({ title: 'Mine', version: 2 });
  });

  it('edits the description in its tab', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.click(panel.getByRole('button', { name: 'Edit' }));
    const editor = panel.getByRole('textbox', { name: 'Description' });
    await userEvent.clear(editor);
    await userEvent.type(editor, 'Eight columns');
    await userEvent.click(panel.getByRole('button', { name: /Save/ }));

    expect(await panel.findByText('Eight columns')).toBeInTheDocument();
    expect(panel.queryByRole('textbox', { name: 'Description' })).not.toBeInTheDocument();
    expect(await lastBody(api.requests(`PATCH ${BASE}`))).toEqual({
      description: 'Eight columns',
      version: 1,
    });
  });

  it('shows the problem when the daemon refuses a move', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.click(panel.getByRole('tab', { name: 'Acceptance' }));
    api.set(`POST ${BASE}/move`, () => problem(422, 'not-needed-human-only', 'people only'));
    await userEvent.selectOptions(panel.getByRole('combobox', { name: 'Status' }), 'Not needed');

    expect(await panel.findByRole('alert')).toHaveTextContent(
      'Only a person can mark a ticket as not needed.',
    );
    expect(panel.getByRole('combobox', { name: 'Status' })).toHaveValue('backlog');
    expect(await lastBody(api.requests(`POST ${BASE}/move`))).toEqual({
      status: 'not-needed',
      version: 1,
    });
  });

  it('names the open dependency when a move to done is refused', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.selectOptions(panel.getByRole('combobox', { name: 'Status' }), 'Done');

    const ask = within(await screen.findByRole('dialog', { name: /How can one tell/ }));
    await userEvent.type(ask.getByRole('textbox', { name: 'Acceptance' }), 'It renders');
    await userEvent.click(ask.getByRole('button', { name: 'Move to done' }));

    expect(await panel.findByRole('alert')).toHaveTextContent(
      'ARC-0001 waits on ARC-0002; finish those first.',
    );
    expect(await lastBody(api.requests(`POST ${BASE}/move`))).toEqual({
      status: 'done',
      version: 1,
      acceptance: 'It renders',
    });
  });

  it('moves the ticket to another column', async () => {
    server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.selectOptions(panel.getByRole('combobox', { name: 'Status' }), 'In progress');

    const progress = screen.getByRole('region', { name: 'In progress' });
    expect(await within(progress).findByText('Show the board')).toBeInTheDocument();
    expect(panel.getByRole('combobox', { name: 'Status' })).toHaveValue('in-progress');
  });

  it('checks a step and adds one', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.click(panel.getByRole('tab', { name: 'Steps (1/2)' }));

    await userEvent.click(panel.getByRole('checkbox', { name: 'Step 2: Write the page' }));
    expect(await panel.findByRole('tab', { name: 'Steps (2/2)' })).toBeInTheDocument();
    expect(panel.getByRole('checkbox', { name: 'Step 2: Write the page' })).toBeChecked();
    expect(await lastBody(api.requests(`PATCH ${BASE}/steps/2`))).toEqual({
      done: true,
      expectText: 'Write the page',
    });

    await userEvent.click(panel.getByRole('button', { name: '+ Step' }));
    const form = within(panel.getByRole('form', { name: 'New step' }));
    await userEvent.type(form.getByRole('textbox', { name: 'Text' }), 'Write the tests{Enter}');

    expect(
      await panel.findByRole('checkbox', { name: 'Step 3: Write the tests' }),
    ).not.toBeChecked();
    expect(panel.queryByRole('form', { name: 'New step' })).not.toBeInTheDocument();
    expect(await lastBody(api.requests(`POST ${BASE}/steps`))).toEqual({ text: 'Write the tests' });
  });

  it('reports a step that changed in the meantime', async () => {
    const { entries } = server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.click(panel.getByRole('tab', { name: 'Steps (1/2)' }));
    const item = entries.get(KEY);
    if (item)
      item.steps = [STEPS[0], { number: 2, text: 'Renamed', done: false }].flatMap((step) =>
        step ? [step] : [],
      );

    await userEvent.click(panel.getByRole('checkbox', { name: 'Step 2: Write the page' }));
    expect(await panel.findByRole('alert')).toHaveTextContent(STEP_CONFLICT_MESSAGE);
    expect(await panel.findByRole('checkbox', { name: 'Step 2: Renamed' })).not.toBeChecked();
  });

  it('deletes the ticket after confirmation', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.click(panel.getByRole('button', { name: 'Delete ticket' }));
    const confirm = within(panel.getByRole('alertdialog', { name: `Delete ${KEY}` }));
    expect(confirm.getByText(/cannot be undone/)).toBeInTheDocument();
    await userEvent.click(confirm.getByRole('button', { name: 'Delete for good' }));

    await waitFor(() => {
      expect(screen.queryByRole('dialog', { name: `Ticket ${KEY}` })).not.toBeInTheDocument();
    });
    const backlog = screen.getByRole('region', { name: 'Backlog' });
    await waitFor(() => {
      expect(within(backlog).queryByText('Show the board')).not.toBeInTheDocument();
    });
    const [request] = api.requests(`DELETE ${BASE}`);
    expect(new URL(request?.url ?? '').search).toBe('?version=1');
  });
});
