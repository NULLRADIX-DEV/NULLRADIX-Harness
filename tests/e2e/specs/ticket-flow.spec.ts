import type { Page } from '@playwright/test';
import { expect, test } from '../support/fixtures';
import { list, record } from '../support/json';
import { seedProject } from '../support/seed';

const TITLE = 'Build the discount';
const ACCEPTANCE = 'The price drops by the discount';

async function dispatcherMenu(page: Page, item: string): Promise<void> {
  await page.getByRole('button', { name: /^Dispatcher:/ }).click();
  await page.getByRole('menuitem', { name: item }).click();
}

test('a ticket is created, dispatched, reviewed and merged', async ({ page, daemon }) => {
  const seed = await seedProject(daemon, { repository: true });
  await page.goto('/board');

  await page.getByRole('button', { name: 'New ticket' }).click();
  const dialog = page.getByRole('dialog', { name: /New ticket/ });
  await dialog.getByLabel('Title').fill(TITLE);
  await dialog.getByLabel('Labels').fill('auto');
  await dialog.getByLabel('Acceptance').fill(ACCEPTANCE);
  await dialog.getByLabel('Needs planning').uncheck();
  await dialog.getByRole('button', { name: 'Create', exact: true }).click();
  await expect(page.getByText('ARC-0001 created.')).toBeVisible();

  // the plan comes from the CLI, as a planning run would write it
  await daemon.cli(['plan', 'write', 'ARC-0001', '--plan', 'Apply it', '--handbook', 'npm test']);
  const card = (column: string) =>
    page.getByRole('region', { name: column }).getByRole('button', { name: new RegExp(TITLE) });
  await expect(card('Planned')).toBeVisible();

  await dispatcherMenu(page, 'Enable');
  await dispatcherMenu(page, 'Run pass now');
  await expect(card('In progress')).toBeVisible();
  await expect
    .poll(async () => {
      const runs = list(
        await daemon.api('GET', '/api/v1/runs?ticketKey=ARC-0001&kind=work'),
        'items',
      );
      return record(runs[0], 'a run')['status'];
    })
    .toBe('succeeded');

  // the fake Claude does not call the CLI, so the test plays the agent that finishes its ticket
  await daemon.cli([
    'ticket',
    'move',
    'ARC-0001',
    'done',
    '--acceptance',
    ACCEPTANCE,
    '--author',
    'worker-ARC-0001',
  ]);
  await dispatcherMenu(page, 'Run pass now');
  await expect(card('Blocked')).toBeVisible();

  await card('Blocked').click();
  const ticket = page.getByRole('dialog', { name: 'Ticket ARC-0001' });
  await ticket.getByRole('tab', { name: 'Review' }).click();
  await expect(
    ticket.getByText('The ticket waits for a review before it is merged.'),
  ).toBeVisible();
  await expect(ticket.getByText('price.ts', { exact: true })).toBeVisible();
  await ticket.getByRole('button', { name: 'Accept', exact: true }).click();

  await expect(card('Done')).toBeVisible();
  // the accept queues the merge; a pass carries it out, and the panel covers the menu
  await ticket.getByRole('button', { name: 'Close' }).click();
  await dispatcherMenu(page, 'Run pass now');
  await card('Done').click();
  await ticket.getByRole('tab', { name: 'Merges' }).click();
  await expect(ticket.getByText('Merged', { exact: true })).toBeVisible();
  const merged = await daemon.git(seed.repository, ['show', 'main:src/price.ts']);
  expect(merged).toContain('0.9');
});
