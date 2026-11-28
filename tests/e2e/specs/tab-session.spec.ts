import { text } from '../support/json';
import { expect, test } from '../support/fixtures';

const KEY_IN_STORAGE = "sessionStorage.getItem('nullradix-tab-key')";

test('a second tab of the same origin works through the key of the first tab', async ({ page }) => {
  const other = await page.context().newPage();
  await other.goto('/board');
  await expect(other.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
  const key = await other.evaluate<string | null>(KEY_IN_STORAGE);
  expect(key).toMatch(/^[0-9a-f]{64}$/);
  expect(await page.evaluate<string | null>(KEY_IN_STORAGE)).toBe(key);
  await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
  await other.close();
});

test('the cookie alone signs nobody in when no tab can share its key', async ({ page }) => {
  const other = await page.context().newPage();
  await other.addInitScript('delete globalThis.BroadcastChannel;');
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
  await expect(form.getByText('needs a confirmation first')).toBeVisible();

  const code = text(await daemon.api('POST', '/api/v1/auth/step-up-codes'), 'code');
  await form.getByRole('textbox', { name: 'Confirmation code' }).fill(code);
  await form.getByRole('button', { name: 'Confirm', exact: true }).click();
  await expect(form.getByText(/Confirmed until/)).toBeVisible();
  await form.getByRole('button', { name: 'Save dispatcher settings' }).click();
  await expect(form.getByText('Saved.')).toBeVisible();
});

test('the link of nullradix confirm confirms the signed-in session from a second tab', async ({
  page,
  daemon,
}) => {
  await page.goto('/settings');
  const code = text(await daemon.api('POST', '/api/v1/auth/step-up-codes'), 'code');
  const opened = await page.context().newPage();
  await opened.goto(`/#confirm=${code}`);
  // the second tab takes the key of the first and redeems the code itself
  await expect(opened.getByText('Confirmed for 10 minutes')).toBeVisible();
  // a reload shows the confirmation: the first tab does not poll the session
  await page.reload();
  await expect(page.getByText(/Confirmed until/)).toBeVisible();
});

test('an account folder outside the accounts folder needs a confirmation', async ({ page }) => {
  await page.goto('/settings');
  const form = page.getByRole('form', { name: 'New account' });
  await form.getByRole('textbox', { name: 'Name', exact: true }).fill('Work');
  await form.getByText('Use an existing Claude folder').click();
  await form.getByRole('textbox', { name: 'Folder' }).fill('/tmp/outside-claude-folder');
  await form.getByRole('button', { name: 'Add account', exact: true }).click();
  await expect(form.getByText(/needs a confirmation: run `nullradix confirm`/)).toBeVisible();
});

test('turning the confirmation off asks for a fresh code', async ({ page }) => {
  await page.goto('/settings');
  const form = page.getByRole('form', { name: 'Dispatcher configuration' });
  await form
    .getByRole('checkbox', { name: 'Ask for a confirmation before changing commands' })
    .click();
  await expect(form.getByText(/A confirmed session is not enough/)).toBeVisible();
  await form.getByRole('textbox', { name: 'Confirmation code' }).fill('0'.repeat(64));
  await form.getByRole('button', { name: 'Turn off' }).click();
  await expect(form.getByText('The confirmation code is unknown or expired')).toBeVisible();
});
