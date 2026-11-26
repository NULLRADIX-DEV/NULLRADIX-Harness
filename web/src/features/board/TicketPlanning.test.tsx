import { screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { boardServer, entry, ticket } from '../../test/boardServer';
import { json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { Run } from '../runs/queries';
import type { Ticket } from '../tickets/queries';

const KEY = 'ARC-0001';
const RUNS = '/api/v1/runs';
const START = `/api/v1/tickets/${KEY}/plan-runs`;

function planRun(overrides: Partial<Run> = {}): Run {
  return {
    id: '0190a0b0-0000-7000-8000-00000000a001',
    label: `plan ${KEY}`,
    status: 'running',
    createdAt: 1,
    version: 1,
    ...overrides,
  };
}

function server(runs: Run[] = [], change: Partial<Ticket> = {}) {
  const fake = boardServer({ entries: [entry(ticket(change))] });
  fake.api.set(`GET ${RUNS}`, () => json(200, { items: runs }));
  return fake;
}

async function openPlanning() {
  const backlog = await screen.findByRole('region', { name: 'Backlog' });
  await userEvent.click(await within(backlog).findByRole('button', { name: /ARC-0001/ }));
  const panel = await screen.findByRole('dialog', { name: `Ticket ${KEY}` });
  await within(panel).findByRole('textbox', { name: 'Title' });
  return within(panel);
}

describe('TicketPlanning', () => {
  it('starts a planning and refuses a second one while it runs', async () => {
    let runs: Run[] = [];
    const { api } = server();
    api.set(`GET ${RUNS}`, () => json(200, { items: runs }));
    api.set(`POST ${START}`, () => {
      runs = [planRun()];
      return json(202, runs[0]);
    });
    renderApp('/board');

    const panel = await openPlanning();
    const button = await panel.findByRole('button', { name: 'Plan' });
    await vi.waitFor(() => {
      expect(button).toBeEnabled();
    });
    const asked = api.requests(`GET ${RUNS}`).map((request) => request.url);
    expect(asked.some((url) => url.includes(`ticketKey=${KEY}`) && url.includes('kind=plan'))).toBe(
      true,
    );
    await userEvent.click(button);

    expect(await panel.findByRole('button', { name: 'Planning...' })).toBeDisabled();
    expect(api.requests(`POST ${START}`)).toHaveLength(1);
  });

  it('waits while a finished planning is not settled', async () => {
    server([planRun({ status: 'succeeded', result: { outcome: null } })]);
    renderApp('/board');

    const panel = await openPlanning();
    expect(await panel.findByRole('button', { name: 'Planning...' })).toBeDisabled();
  });

  it('offers the planning again once the run is settled', async () => {
    server([planRun({ status: 'succeeded', result: { outcome: 'planned' } })]);
    renderApp('/board');

    const panel = await openPlanning();
    const button = await panel.findByRole('button', { name: 'Plan' });
    await vi.waitFor(() => {
      expect(button).toBeEnabled();
    });
  });

  it('says why a planning could not start', async () => {
    const { api } = server();
    api.set(`POST ${START}`, () => problem(422, 'plan-waiting-question', 'waits on a question'));
    renderApp('/board');

    const panel = await openPlanning();
    const button = await panel.findByRole('button', { name: 'Plan' });
    await vi.waitFor(() => {
      expect(button).toBeEnabled();
    });
    await userEvent.click(button);

    expect(await panel.findByRole('alert')).toHaveTextContent('An open question holds this ticket');
    expect(button).toBeEnabled();
  });

  it('offers no planning on a ticket that is worked on or done', async () => {
    server([], { status: 'in-progress', column: 'in-progress' });
    renderApp('/board');

    const columns = await screen.findByRole('region', { name: 'In progress' });
    await userEvent.click(await within(columns).findByRole('button', { name: /ARC-0001/ }));
    const panel = within(await screen.findByRole('dialog', { name: `Ticket ${KEY}` }));
    await panel.findByRole('textbox', { name: 'Title' });
    expect(panel.queryByRole('button', { name: /Plan/ })).not.toBeInTheDocument();
  });
});
