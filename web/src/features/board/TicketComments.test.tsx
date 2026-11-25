import { screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { boardServer, comment, entry, ticket } from '../../test/boardServer';
import { problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { initials } from './TicketComments';

const KEY = 'ARC-0001';
const BASE = `/api/v1/tickets/${KEY}`;

function server() {
  return boardServer({
    entries: [
      entry(
        ticket(),
        [],
        [],
        [
          comment(KEY, 1, 'Please check the login'),
          comment(KEY, 2, 'Step 1 done\n  tests pass', 'claude'),
        ],
      ),
    ],
  });
}

async function openComments() {
  const backlog = await screen.findByRole('region', { name: 'Backlog' });
  await userEvent.click(await within(backlog).findByRole('button', { name: /ARC-0001/ }));
  const panel = within(await screen.findByRole('dialog', { name: `Ticket ${KEY}` }));
  await userEvent.click(await panel.findByRole('tab', { name: 'Comments (2)' }));
  return panel;
}

describe('TicketComments', () => {
  it('shows the comments newest first with author and the text as written', async () => {
    server();
    renderApp('/board');
    const panel = await openComments();

    const items = within(panel.getByRole('list')).getAllByRole('listitem');
    expect(items.map((item) => within(item).getByRole('strong').textContent)).toEqual([
      'claude',
      'Ben',
    ]);
    const body = within(items[0] ?? document.body).getByText(/Step 1 done/);
    expect(body.textContent).toBe('Step 1 done\n  tests pass');
    expect(within(items[0] ?? document.body).getByText('agent')).toBeInTheDocument();
    expect(within(items[1] ?? document.body).queryByText('agent')).not.toBeInTheDocument();
  });

  it('adds a comment with Ctrl+Enter and empties the box', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openComments();
    const box = panel.getByRole('textbox', { name: 'New comment' });

    await userEvent.type(box, 'Looks good{Control>}{Enter}{/Control}');

    expect(await panel.findByRole('tab', { name: 'Comments (3)' })).toBeInTheDocument();
    expect(await panel.findByText('Looks good')).toBeInTheDocument();
    expect(box).toHaveValue('');
    expect(panel.getByText(/Version 2/)).toBeInTheDocument();
    const [request] = api.requests(`POST ${BASE}/comments`);
    expect(await request?.json()).toEqual({ body: 'Looks good' });
  });

  it('refuses an empty comment without asking the daemon', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openComments();
    const box = panel.getByRole('textbox', { name: 'New comment' });

    await userEvent.type(box, '   {Control>}{Enter}{/Control}');

    expect(panel.getByRole('button', { name: 'Comment' })).toBeDisabled();
    expect(api.requests(`POST ${BASE}/comments`)).toHaveLength(0);
  });

  it('shows the problem when the daemon refuses a comment and keeps the text', async () => {
    const { api } = server();
    renderApp('/board');
    const panel = await openComments();
    api.set(`POST ${BASE}/comments`, () => problem(422, 'comment-empty', 'the comment is empty'));
    const box = panel.getByRole('textbox', { name: 'New comment' });

    await userEvent.type(box, 'Only spaces to the daemon');
    await userEvent.click(panel.getByRole('button', { name: 'Comment' }));

    expect(await panel.findByRole('alert')).toHaveTextContent('A comment needs a text.');
    expect(box).toHaveValue('Only spaces to the daemon');
  });

  it('makes initials from one or two words', () => {
    expect(initials('Florian')).toBe('FL');
    expect(initials('benjamin.biber')).toBe('BB');
    expect(initials('  ')).toBe('?');
  });
});
