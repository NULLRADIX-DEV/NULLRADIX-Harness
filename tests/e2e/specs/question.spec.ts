import { expect, test } from '../support/fixtures';
import { seedProject } from '../support/seed';

test('an open question is answered with an option', async ({ page, daemon }) => {
  const seed = await seedProject(daemon);
  await daemon.api('POST', `/api/v1/projects/${seed.projectId}/questions`, {
    title: 'Which store?',
    options: [
      { title: 'File', text: 'one JSON file' },
      { title: 'Db', text: 'SQLite next to the daemon', recommended: true },
    ],
  });

  await page.goto('/questions');
  await page.getByRole('button', { name: /Which store\?/ }).click();
  const decide = page.getByRole('region', { name: 'Decide' });
  await decide.getByRole('radio', { name: /Db/ }).check();
  await decide.getByLabel('Reason').fill('It needs no extra service');
  await decide.getByRole('button', { name: /^Answer/ }).click();

  await expect(page.getByRole('tab', { name: 'Open (0)' })).toBeVisible();
  await page.getByRole('tab', { name: 'Answered (1)' }).click();
  await expect(page.getByRole('button', { name: /Which store\?/ })).toContainText('Answered');
  const found = await daemon.api(
    'GET',
    `/api/v1/projects/${seed.projectId}/questions?state=answered`,
  );
  expect(JSON.stringify(found)).toContain('Which store?');
});
