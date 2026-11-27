import { text } from '../support/json';
import { expect, test } from '../support/fixtures';

test('a tab without the key of the session is asked to sign in again', async ({ page }) => {
  // the cookie is shared, the key lives in the session storage of the tab that signed in
  const other = await page.context().newPage();
  await other.goto('/board');
  await expect(
    other.getByRole('heading', { name: 'Open NULLRADIX from your terminal' }),
  ).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
  await other.close();
});

test('the key leaves the address and the page keeps working after a reload', async ({ page }) => {
  expect(page.url()).not.toContain('#k=');
  await page.reload();
  await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
});

test('a change of the Claude command needs a confirmation and works once it is given', async ({
  page,
  daemon,
}) => {
  await page.goto('/settings');
  const form = page.getByRole('form', { name: 'Dispatcher configuration' });
  const command = form.getByRole('textbox', { name: 'Claude command' });
  await command.fill('/opt/claude');
  await form.getByRole('button', { name: 'Save dispatcher settings' }).click();
  await expect(form.getByText('The fields that run commands are marked below.')).toBeVisible();
  await expect(
    form.getByText('confirm with `nullradix confirm` before changing this'),
  ).toBeVisible();

  const code = text(await daemon.api('POST', '/api/v1/auth/step-up-codes'), 'code');
  await form.getByRole('textbox', { name: 'Confirmation code' }).fill(code);
  await form.getByRole('button', { name: 'Confirm', exact: true }).click();
  await expect(form.getByText(/Confirmed until/)).toBeVisible();
  await form.getByRole('button', { name: 'Save dispatcher settings' }).click();
  await expect(form.getByText('Saved.')).toBeVisible();
});

test('the link of nullradix confirm confirms the signed-in tab from another tab', async ({
  page,
  daemon,
}) => {
  await page.goto('/settings');
  const code = text(await daemon.api('POST', '/api/v1/auth/step-up-codes'), 'code');
  const opened = await page.context().newPage();
  await opened.goto(`/#confirm=${code}`);
  await expect(opened.getByText('Confirmed in your open NULLRADIX tab')).toBeVisible();
  await expect(page.getByText('Confirmed for 10 minutes')).toBeVisible();
  await expect(page.getByText(/Confirmed until/)).toBeVisible();
});
