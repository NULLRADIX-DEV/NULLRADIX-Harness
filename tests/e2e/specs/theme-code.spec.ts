import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { Page } from '@playwright/test';
import type { Daemon } from '../support/daemon';
import { expect, test } from '../support/fixtures';

const MANIFEST = {
  id: 'night-code',
  name: 'Night Code',
  version: '1.0.0',
  author: 'Ada',
  sdkVersion: '^1.3',
  colorScheme: 'dark',
  slots: [],
  permissions: { network: [] },
};

// It takes React and the SDK from the host, so it runs only if the import map and the CSP work.
const MAIN = `import React from 'react';
import { SDK_VERSION } from '@nullradix/theme-sdk';
document.documentElement.dataset.themeCode = React.version + ' ' + SDK_VERSION;
export default { id: 'night-code', slots: {} };
`;

// Only the person can allow code, so the package goes in through the app and its confirm dialog.
async function installAsPerson(page: Page, pack: string, title: string): Promise<void> {
  await page.goto('/settings');
  await page.getByLabel('Install a theme package').setInputFiles(pack);
  const dialog = page.getByRole('alertdialog', { name: title });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('This package contains code');
  await dialog.getByRole('button', { name: /^(Install|Update)$/ }).click();
  await expect(dialog).toBeHidden();
}

async function installThemeWithCode(page: Page, daemon: Daemon): Promise<void> {
  const dir = join(daemon.work, 'night-code');
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, 'manifest.json'), JSON.stringify(MANIFEST));
  await writeFile(
    join(dir, 'theme.css'),
    '[data-theme-mode="night-code"] { --accent: #6c63ff; }\n',
  );
  await writeFile(join(dir, 'main.js'), MAIN);
  const pack = join(daemon.work, 'night-code.nullradix');
  await daemon.cli(['theme', 'pack', dir, '--out', pack]);
  await installAsPerson(page, pack, 'Install Night Code 1.0.0?');
  await daemon.api('PUT', '/api/v1/settings/theme', { theme: 'night-code' });
}

test('theme code runs only after its switch is on, and not in safe mode', async ({
  page,
  daemon,
}) => {
  await installThemeWithCode(page, daemon);
  const root = page.locator('html');

  await page.goto('/settings');
  await expect(root).toHaveAttribute('data-theme-mode', 'night-code');
  await expect(root).not.toHaveAttribute('data-theme-code', /.*/);

  await page.getByRole('switch', { name: 'Run the code of Night Code' }).click();
  await expect(page.getByRole('switch', { name: 'Run the code of Night Code' })).toBeChecked();

  await page.reload();
  await expect(root).toHaveAttribute('data-theme-code', /^19\.\d+\.\d+ 1\.\d+\.\d+$/);

  await page.goto('/settings?safe-theme');
  await expect(page.locator('.theme-notice')).toContainText('Safe mode');
  await expect(root).not.toHaveAttribute('data-theme-code', /.*/);
  await page.getByRole('button', { name: 'Start normally' }).click();
  await expect(root).toHaveAttribute('data-theme-code', /^19\./);
});

test('an update that adds a network domain turns the switch off again', async ({
  page,
  daemon,
}) => {
  await installThemeWithCode(page, daemon);
  await page.goto('/settings');
  await page.getByRole('switch', { name: 'Run the code of Night Code' }).click();
  await expect(page.getByRole('switch', { name: 'Run the code of Night Code' })).toBeChecked();

  const dir = join(daemon.work, 'night-code');
  const next = { ...MANIFEST, version: '1.1.0', permissions: { network: ['cdn.example.com'] } };
  await writeFile(join(dir, 'manifest.json'), JSON.stringify(next));
  const pack = join(daemon.work, 'night-code-2.nullradix');
  await daemon.cli(['theme', 'pack', dir, '--out', pack]);
  await installAsPerson(page, pack, 'Update Night Code 1.1.0?');

  // the list refreshes through the change stream, with no reload
  await expect(page.getByRole('switch', { name: 'Run the code of Night Code' })).not.toBeChecked();
  await page.reload();
  await expect(page.locator('html')).not.toHaveAttribute('data-theme-code', /.*/);
});
