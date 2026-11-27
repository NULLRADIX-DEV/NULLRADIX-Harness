import { expect, test } from '../support/fixtures';
import { text } from '../support/json';

test('a run that is under way survives a daemon restart', async ({ page, daemon }) => {
  const started = await daemon.api('POST', '/api/v1/runs', {
    label: 'Slow job',
    program: process.execPath,
    args: ['-e', "console.log('begin'); setTimeout(() => { console.log('end'); }, 8000);"],
    cwd: daemon.work,
    timeoutSeconds: 60,
  });
  const id = text(started, 'id');

  await page.goto('/dashboard');
  const row = page.getByRole('list', { name: 'Runs' }).getByRole('button', { name: /Slow job/ });
  await expect(row).toContainText('Running');

  await daemon.restart();

  await expect(row).toContainText('Succeeded');
  await row.click();
  await expect(page.getByRole('log', { name: 'Output of Slow job' })).toContainText('end');
  expect(text(await daemon.api('GET', `/api/v1/runs/${id}`), 'status')).toBe('succeeded');
});
