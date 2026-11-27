import type { Page } from '@playwright/test';
import { expect } from './fixtures';

// Only the person can allow code, so the package goes in through the app and its confirm dialog.
export async function installAsPerson(page: Page, pack: string, title: string): Promise<void> {
  await page.goto('/settings');
  await page.getByLabel('Install a theme package').setInputFiles(pack);
  const dialog = page.getByRole('alertdialog', { name: title });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('This package contains code');
  await dialog.getByRole('button', { name: /^(Install|Update)$/ }).click();
  await expect(dialog).toBeHidden();
}
