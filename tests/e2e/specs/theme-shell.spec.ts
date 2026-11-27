import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { Page } from '@playwright/test';
import type { Daemon } from '../support/daemon';
import { expect, test } from '../support/fixtures';
import { seedProject } from '../support/seed';

const MANIFEST = {
  id: 'night-shell',
  name: 'Night Shell',
  version: '1.0.0',
  author: 'Ada',
  sdkVersion: '^2.0',
  colorScheme: 'dark',
  slots: ['Shell', 'page:dashboard'],
  permissions: { network: [] },
};

// A shell and a dashboard page made only from the SDK: nav, boards and the page of the route.
const MAIN = `import React from 'react';
import { NavLink, Outlet, useBoards, useNav } from '@nullradix/theme-sdk';
const h = React.createElement;

function Shell() {
  const nav = useNav();
  const boards = useBoards();
  const choose = (event) => {
    const board = boards.boards.find((each) => each.id === event.target.value);
    if (board) boards.choose({ project: board.project, board: board.id });
  };
  return h(
    'div',
    { 'data-themed-shell': '' },
    h('nav', { 'aria-label': 'Night navigation' }, nav.map((item) => h(NavLink, { key: item.id, id: item.id }))),
    h(
      'label',
      null,
      'Night board',
      h(
        'select',
        { value: boards.current ? boards.current.board : '', onChange: choose },
        boards.boards.map((board) => h('option', { key: board.id, value: board.id }, board.project + ' / ' + board.name)),
      ),
    ),
    h('main', null, h(Outlet)),
  );
}

function Dashboard({ pid }) {
  return h('h1', { 'data-themed-dashboard': pid }, 'Night dashboard of ' + pid);
}

export default { id: 'night-shell', slots: { Shell, 'page:dashboard': Dashboard } };
`;

async function installAndEnable(daemon: Daemon, page: Page): Promise<void> {
  const dir = join(daemon.work, 'night-shell');
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, 'manifest.json'), JSON.stringify(MANIFEST));
  await writeFile(
    join(dir, 'theme.css'),
    '[data-theme-mode="night-shell"] { --accent: #6c63ff; }\n',
  );
  await writeFile(join(dir, 'main.js'), MAIN);
  const pack = join(daemon.work, 'night-shell.nullradix');
  await daemon.cli(['theme', 'pack', dir, '--out', pack]);
  await daemon.cli(['theme', 'install', pack, '--yes']);
  await daemon.api('PUT', '/api/v1/settings/theme', { theme: 'night-shell' });
  await page.goto('/settings');
  await page.getByRole('switch', { name: 'Run the code of Night Shell' }).click();
  await expect(page.getByRole('switch', { name: 'Run the code of Night Shell' })).toBeChecked();
}

test('a theme replaces the shell and the dashboard, and safe mode brings the app back', async ({
  page,
  daemon,
}) => {
  const seed = await seedProject(daemon);
  await daemon.api('POST', `/api/v1/projects/${seed.projectId}/boards`, { name: 'Side' });
  await installAndEnable(daemon, page);

  await page.goto('/board');
  const shell = page.locator('[data-themed-shell]');
  await expect(shell).toBeVisible();
  await expect(page.locator('.shell-sidebar')).toHaveCount(0);
  const nav = page.getByRole('navigation', { name: 'Night navigation' });
  await expect(nav.getByRole('link')).toHaveText([
    'Board',
    'Inbox',
    'Dashboard',
    'Questions',
    'Sessions',
    'Settings',
  ]);
  await expect(nav.getByRole('link', { name: 'Board', exact: true })).toHaveAttribute(
    'aria-current',
    'page',
  );

  // the page of the route shows inside the theme shell
  await expect(shell.getByRole('heading', { level: 1, name: 'Board', exact: true })).toBeVisible();

  // a board chosen through the host is the board the app shows and remembers
  const night = page.getByLabel('Night board');
  await expect(night.locator('option:checked')).toHaveText('ARC / Main');
  await night.selectOption({ label: 'ARC / Side' });
  await expect(shell.getByRole('combobox', { name: 'Board', exact: true })).toHaveValue(
    await night.inputValue(),
  );
  await page.reload();
  await expect(page.getByLabel('Night board').locator('option:checked')).toHaveText('ARC / Side');

  // the dashboard is the page of the theme
  await nav.getByRole('link', { name: 'Dashboard' }).click();
  await expect(page.locator('[data-themed-dashboard="ARC"]')).toHaveText('Night dashboard of ARC');
  await expect(page.getByRole('heading', { level: 1, name: 'Dashboard', exact: true })).toHaveCount(
    0,
  );

  // the way out is outside the theme shell: the settings in the built-in shell
  const escape = page.getByRole('navigation', { name: 'Leave theme' });
  await escape.getByRole('link', { name: 'Settings' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
  await expect(shell).toHaveCount(0);
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('link', {
      name: 'Dashboard',
    })
    .click();
  await expect(shell).toBeVisible();

  // safe mode removes the theme code, shell and page
  await page
    .getByRole('navigation', { name: 'Leave theme' })
    .getByRole('button', {
      name: 'Safe mode',
    })
    .click();
  await expect(page.locator('.theme-notice')).toContainText('Safe mode');
  await expect(shell).toHaveCount(0);
  await expect(page.locator('[data-themed-dashboard]')).toHaveCount(0);
  await expect(
    page.getByRole('heading', { level: 1, name: 'Dashboard', exact: true }),
  ).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Leave theme' })).toHaveCount(0);

  await page.getByRole('button', { name: 'Start normally' }).click();
  await expect(shell).toBeVisible();
  await expect(page.locator('[data-themed-dashboard="ARC"]')).toBeVisible();
});

test('a theme shell that fails leaves the built-in shell and one notice', async ({
  page,
  daemon,
}) => {
  await seedProject(daemon);
  const dir = join(daemon.work, 'night-shell');
  await mkdir(dir, { recursive: true });
  await writeFile(join(dir, 'manifest.json'), JSON.stringify(MANIFEST));
  await writeFile(
    join(dir, 'theme.css'),
    '[data-theme-mode="night-shell"] { --accent: #6c63ff; }\n',
  );
  await writeFile(
    join(dir, 'main.js'),
    `const Shell = () => { throw new Error('broken shell'); };
export default { id: 'night-shell', slots: { Shell } };
`,
  );
  const pack = join(daemon.work, 'night-shell.nullradix');
  await daemon.cli(['theme', 'pack', dir, '--out', pack]);
  await daemon.cli(['theme', 'install', pack, '--yes']);
  await daemon.api('PUT', '/api/v1/settings/theme', { theme: 'night-shell' });
  await page.goto('/settings');
  await page.getByRole('switch', { name: 'Run the code of Night Shell' }).click();
  await expect(page.getByRole('switch', { name: 'Run the code of Night Shell' })).toBeChecked();

  await page.reload();
  await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
  await expect(page.locator('.theme-notice')).toHaveCount(1);
  await expect(page.locator('.theme-notice')).toContainText('"night-shell"');
  await page
    .getByRole('navigation', { name: 'Main navigation' })
    .getByRole('link', { name: 'Board', exact: true })
    .click();
  await expect(page.getByRole('heading', { level: 1, name: 'Board', exact: true })).toBeVisible();
});
