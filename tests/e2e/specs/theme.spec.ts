import { expect, test } from '../support/fixtures';

test('the chosen theme applies at once and survives a reload', async ({ page }) => {
  const root = page.locator('html');
  await page.goto('/settings');
  await expect(root).toHaveAttribute('data-theme', 'dark');

  await page.getByRole('combobox', { name: 'Theme' }).selectOption({ label: 'Light' });
  await expect(root).toHaveAttribute('data-theme', 'light');

  await page.reload();
  await expect(root).toHaveAttribute('data-theme', 'light');
  await expect(page.getByRole('combobox', { name: 'Theme' })).toHaveValue('light');

  await page.getByRole('combobox', { name: 'Theme' }).selectOption({ label: 'Dark' });
  await expect(root).toHaveAttribute('data-theme', 'dark');
});
