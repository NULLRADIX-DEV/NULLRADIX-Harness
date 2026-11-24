import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { USER_ID, boardServer, entry, step, stepId, ticket } from '../../test/boardServer';
import { FakeEventSource } from '../../test/eventSource';
import { json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { HistoryLine, Step, Ticket } from '../tickets/queries';
import { CONFLICT_MESSAGE, STEP_CONFLICT_MESSAGE } from './messages';

const KEY = 'ARC-0001';
const BASE = `/api/v1/tickets/${KEY}`;

const STEPS: Step[] = [step(1, 'Write the queries', true), step(2, 'Write the page')];

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

// Another client changes the ticket and the change feed tells this one.
async function changeElsewhere(fake: ReturnType<typeof server>, change: Partial<Ticket>) {
  fake.bumpElsewhere(KEY, change);
  act(() => {
    FakeEventSource.latest().emit('change', { seq: 1, entity: 'ticket' });
  });
  await screen.findByText(/Version 2/);
}

function fieldConflict(label: string): string {
  return `${label} changed in the meantime. Your text is kept; save again to replace it.`;
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
    expect(await lastBody(api.requests(`PATCH ${BASE}/steps/${stepId(2)}`))).toEqual({
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
    if (item) item.steps = [STEPS[0], step(2, 'Renamed')].flatMap((each) => (each ? [each] : []));

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

  it('keeps a title typed over a change elsewhere and replaces it on the second save', async () => {
    const fake = server();
    renderApp('/board');
    const panel = await openTicket();
    const title = panel.getByRole('textbox', { name: 'Title' });
    await userEvent.clear(title);
    await userEvent.type(title, 'Mine');
    await changeElsewhere(fake, { title: 'Theirs' });
    expect(title).toHaveValue('Mine');

    await userEvent.type(title, '{Enter}');
    expect(await panel.findByRole('alert')).toHaveTextContent(fieldConflict('Title'));
    expect(fake.api.requests(`PATCH ${BASE}`)).toHaveLength(0);
    expect(title).toHaveValue('Mine');

    await userEvent.type(title, '{Enter}');
    expect(await panel.findByText(/Version 3/)).toBeInTheDocument();
    expect(panel.queryByRole('alert')).not.toBeInTheDocument();
    expect(await lastBody(fake.api.requests(`PATCH ${BASE}`))).toEqual({
      title: 'Mine',
      version: 2,
    });
  });

  it('saves a field when only another field changed elsewhere', async () => {
    const fake = server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.type(panel.getByRole('textbox', { name: 'Assignee' }), 'Ann');
    await changeElsewhere(fake, { labels: ['other'] });
    await userEvent.tab();

    expect(await panel.findByText(/Version 3/)).toBeInTheDocument();
    expect(await lastBody(fake.api.requests(`PATCH ${BASE}`))).toEqual({
      assignee: 'Ann',
      version: 2,
    });
  });

  it('keeps a description typed over a change elsewhere', async () => {
    const fake = server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.click(panel.getByRole('button', { name: 'Edit' }));
    const editor = panel.getByRole('textbox', { name: 'Description' });
    await userEvent.clear(editor);
    await userEvent.type(editor, 'Mine');
    await changeElsewhere(fake, { description: 'Theirs' });

    await userEvent.click(panel.getByRole('button', { name: /Save/ }));
    expect(await panel.findByRole('alert')).toHaveTextContent(fieldConflict('Description'));
    expect(fake.api.requests(`PATCH ${BASE}`)).toHaveLength(0);
    expect(editor).toHaveValue('Mine');

    await userEvent.click(panel.getByRole('button', { name: /Save/ }));
    expect(await panel.findByText('Mine')).toBeInTheDocument();
    expect(await lastBody(fake.api.requests(`PATCH ${BASE}`))).toEqual({
      description: 'Mine',
      version: 2,
    });
  });

  it('keeps the panel and a changed text open on Escape', async () => {
    server();
    renderApp('/board');
    const panel = await openTicket();
    await userEvent.click(panel.getByRole('button', { name: 'Edit' }));
    await userEvent.type(panel.getByRole('textbox', { name: 'Description' }), ' more');
    await userEvent.keyboard('{Escape}');

    expect(screen.getByRole('dialog', { name: `Ticket ${KEY}` })).toBeInTheDocument();
    expect(panel.getByRole('textbox', { name: 'Description' })).toHaveValue(
      'Columns and cards more',
    );

    await userEvent.click(panel.getByRole('button', { name: 'Cancel' }));
    await userEvent.click(panel.getByRole('button', { name: 'Edit' }));
    await userEvent.keyboard('{Escape}');
    expect(panel.queryByRole('textbox', { name: 'Description' })).not.toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: `Ticket ${KEY}` })).toBeInTheDocument();
  });

  it('keeps a busy control focusable and ignores it until the change is saved', async () => {
    const fake = server();
    const { api } = fake;
    renderApp('/board');
    const panel = await openTicket();
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    // the daemon answers only once the test releases it
    api.set(`PATCH ${BASE}`, async () => {
      await held;
      fake.bumpElsewhere(KEY, { needsPlanning: true });
      const item = fake.entries.get(KEY);
      return item ? json(200, item) : problem(404, 'ticket-not-found', KEY);
    });
    await userEvent.click(panel.getByRole('tab', { name: 'Steps (1/2)' }));
    const planning = panel.getByRole('checkbox', { name: 'Needs planning' });
    await userEvent.click(planning);

    await waitFor(() => {
      expect(planning).toHaveAttribute('aria-disabled', 'true');
    });
    expect(planning).not.toBeDisabled();
    expect(planning).toHaveFocus();
    const box = panel.getByRole('checkbox', { name: 'Step 2: Write the page' });
    expect(box).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(box);
    expect(api.requests(`PATCH ${BASE}/steps/${stepId(2)}`)).toHaveLength(0);
    expect(box).not.toBeChecked();
    expect(box).toHaveFocus();

    release();
    expect(await panel.findByText(/Version 2/)).toBeInTheDocument();
    expect(planning).toBeChecked();
    expect(planning).toHaveAttribute('aria-disabled', 'false');
  });

  it('retries a refused delete with the new version', async () => {
    const fake = server();
    renderApp('/board');
    const panel = await openTicket();
    fake.bumpElsewhere(KEY, { description: 'Changed elsewhere' });
    await userEvent.click(panel.getByRole('button', { name: 'Delete ticket' }));
    const confirm = within(panel.getByRole('alertdialog', { name: `Delete ${KEY}` }));
    await userEvent.click(confirm.getByRole('button', { name: 'Delete for good' }));

    expect(await panel.findByRole('alert')).toHaveTextContent(CONFLICT_MESSAGE);
    expect(await panel.findByText(/Version 2/)).toBeInTheDocument();
    await userEvent.click(confirm.getByRole('button', { name: 'Delete for good' }));

    await waitFor(() => {
      expect(screen.queryByRole('dialog', { name: `Ticket ${KEY}` })).not.toBeInTheDocument();
    });
    const [, retry] = fake.api.requests(`DELETE ${BASE}`);
    expect(new URL(retry?.url ?? '').search).toBe('?version=2');
  });
});
