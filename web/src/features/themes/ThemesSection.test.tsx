import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import {
  INSTALLED_STORAGE_KEY,
  THEME_LINK_ID,
  THEME_STORAGE_KEY,
  removeThemeStylesheet,
} from '../../shared/lib/theme';
import type { Theme } from '../../shared/query/themes';
import { FakeEventSource } from '../../test/eventSource';
import { confirmation, fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';

function theme(overrides: Partial<Theme> = {}): Theme {
  return {
    id: 'night-blue',
    name: 'Night Blue',
    packageVersion: '1.2.0',
    author: 'Ada',
    sdkVersion: '^1.3',
    colorScheme: 'dark',
    startColor: '#101018',
    previewUrl: '/api/v1/themes/night-blue/files/assets/preview.png',
    styleUrl: '/api/v1/themes/night-blue/files/theme.css?v=1',
    slots: [],
    network: [],
    settings: [],
    claudeTheme: false,
    hasCode: false,
    codeEnabled: false,
    installedAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

// In-memory daemon for the theme routes.
function themesServer(initial: Theme[] = [], initialActive = 'nullradix') {
  let items = [...initial];
  let active = initialActive;
  const api = fakeApi({
    'GET /api/v1/themes': () => json(200, { items, active }),
    'PUT /api/v1/settings/theme': async (request) => {
      const body = (await request.json()) as { theme: string };
      const known =
        ['nullradix', 'system', 'light', 'dark'].includes(body.theme) ||
        items.some((item) => item.id === body.theme);
      if (!known) return problem(404, 'theme-not-found', 'not installed');
      active = body.theme;
      return json(200, body);
    },
    'POST /api/v1/themes': async (request) => {
      const bytes = await request.arrayBuffer();
      if (bytes.byteLength === 0) return problem(422, 'theme-package-invalid', 'empty');
      const installed = theme();
      items = [...items.filter((item) => item.id !== installed.id), installed];
      return json(201, installed);
    },
    'PUT /api/v1/themes/night-blue/code': async (request) => {
      const body = (await request.json()) as { enabled: boolean; version: number };
      const current = items.find((item) => item.id === 'night-blue');
      if (!current) return problem(404, 'theme-not-found', 'gone');
      if (current.version !== body.version) return problem(409, 'version-conflict', 'stale');
      const next = { ...current, codeEnabled: body.enabled };
      items = items.map((item) => (item.id === next.id ? next : item));
      return json(200, next);
    },
    'DELETE /api/v1/themes/night-blue': (request) => {
      const version = Number(new URL(request.url).searchParams.get('version'));
      const current = items.find((item) => item.id === 'night-blue');
      if (!current) return problem(404, 'theme-not-found', 'gone');
      if (current.version !== version) return problem(409, 'version-conflict', 'stale');
      items = items.filter((item) => item.id !== 'night-blue');
      if (active === 'night-blue') active = 'nullradix';
      return new Response(null, { status: 204 });
    },
  });
  return Object.assign(api, {
    replace(next: Theme[]) {
      items = next;
    },
  });
}

function themeLink(): HTMLLinkElement | null {
  const link = document.getElementById(THEME_LINK_ID);
  return link instanceof HTMLLinkElement ? link : null;
}

afterEach(() => {
  removeThemeStylesheet();
  delete document.documentElement.dataset['theme'];
  delete document.documentElement.dataset['themeMode'];
});

describe('safe mode', () => {
  const stored = JSON.stringify({
    id: 'night-blue',
    scheme: 'dark',
    styleUrl: '/api/v1/themes/night-blue/files/theme.css?v=1',
    startColor: null,
  });

  afterEach(() => {
    window.history.replaceState(null, '', '/');
  });

  it('shows an installed theme by the system without its CSS and keeps the stored look', async () => {
    window.history.replaceState(null, '', '/settings?safe-theme');
    window.localStorage.setItem(THEME_STORAGE_KEY, 'night-blue');
    window.localStorage.setItem(INSTALLED_STORAGE_KEY, stored);
    themesServer([theme()], 'night-blue');
    renderApp('/settings');
    await screen.findByRole('option', { name: 'Night Blue' });
    const root = document.documentElement;
    expect(root.dataset['themeMode']).toBe('night-blue');
    expect(root.dataset['theme']).toBe('light');
    expect(themeLink()).toBeNull();
    expect(window.localStorage.getItem(INSTALLED_STORAGE_KEY)).toBe(stored);
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe('night-blue');
  });

  it('does not touch built-in themes', async () => {
    window.history.replaceState(null, '', '/settings?safe-theme');
    themesServer([theme()], 'dark');
    renderApp('/settings');
    await waitFor(() => {
      expect(document.documentElement.dataset['themeMode']).toBe('dark');
    });
    expect(document.documentElement.dataset['theme']).toBe('dark');
  });
});

describe('ThemesSection', () => {
  it('says so when no package is installed', async () => {
    themesServer();
    renderApp('/settings');
    expect(await screen.findByText(/No theme package is installed/)).toBeInTheDocument();
  });

  it('lists installed themes and offers them in the theme selector', async () => {
    themesServer([theme()]);
    renderApp('/settings');
    const list = screen.getByRole('list', { name: 'Installed themes' });
    expect(await within(list).findByText('Night Blue')).toBeInTheDocument();
    expect(within(list).getByText(/Ada - v1.2.0 - dark/)).toBeInTheDocument();
    const select = screen.getByRole('combobox', { name: 'Theme' });
    expect(within(select).getByRole('option', { name: 'Night Blue' })).toBeInTheDocument();
  });

  it('installs a package from a file and refuses a bad one with the reason', async () => {
    const api = themesServer();
    renderApp('/settings');
    await screen.findByText(/No theme package is installed/);
    const input = screen.getByLabelText('Install a theme package');
    await userEvent.upload(input, new File(['zip'], 'night-blue.nullradix'));
    const list = screen.getByRole('list', { name: 'Installed themes' });
    expect(await within(list).findByText('Night Blue')).toBeInTheDocument();
    const [request] = api.requests('POST /api/v1/themes');
    expect(request?.headers.get('content-type')).toBe('application/octet-stream');
    expect(await request?.clone().text()).toBe('zip');

    await userEvent.upload(input, new File([], 'empty.nullradix'));
    expect(await screen.findByText('This is not a valid theme package.')).toBeInTheDocument();
  });

  it('removes a theme after asking and reports a stale version', async () => {
    const api = themesServer([theme()]);
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Remove Night Blue' }));
    await userEvent.click(screen.getByRole('button', { name: 'Keep' }));
    expect(api.requests('DELETE /api/v1/themes/night-blue')).toHaveLength(0);

    await userEvent.click(screen.getByRole('button', { name: 'Remove Night Blue' }));
    const group = screen.getByRole('group', { name: 'Remove Night Blue' });
    await userEvent.click(within(group).getByRole('button', { name: 'Remove' }));
    const list = screen.getByRole('list', { name: 'Installed themes' });
    await waitFor(() => {
      expect(within(list).queryByText('Night Blue')).not.toBeInTheDocument();
    });
    const [request] = api.requests('DELETE /api/v1/themes/night-blue');
    expect(new URL(request?.url ?? '').searchParams.get('version')).toBe('1');
  });

  it('shows an installed theme when it is chosen and goes back to a built-in one', async () => {
    const api = themesServer([theme()]);
    renderApp('/settings');
    const select = await screen.findByRole('combobox', { name: 'Theme' });
    await screen.findByRole('option', { name: 'Night Blue' });
    await userEvent.selectOptions(select, 'night-blue');

    const root = document.documentElement;
    expect(root.dataset['theme']).toBe('dark');
    expect(root.dataset['themeMode']).toBe('night-blue');
    expect(themeLink()?.getAttribute('href')).toBe('/api/v1/themes/night-blue/files/theme.css?v=1');
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe('night-blue');
    expect(JSON.parse(window.localStorage.getItem(INSTALLED_STORAGE_KEY) ?? 'null')).toEqual({
      id: 'night-blue',
      scheme: 'dark',
      styleUrl: '/api/v1/themes/night-blue/files/theme.css?v=1',
      startColor: '#101018',
    });
    const [put] = api.requests('PUT /api/v1/settings/theme');
    expect(await put?.clone().json()).toEqual({ theme: 'night-blue' });

    await userEvent.selectOptions(select, 'light');
    expect(root.dataset['theme']).toBe('light');
    expect(themeLink()).toBeNull();
    expect(window.localStorage.getItem(INSTALLED_STORAGE_KEY)).toBeNull();
  });

  it('adopts the theme the daemon has chosen', async () => {
    themesServer([theme({ colorScheme: 'light' })], 'night-blue');
    renderApp('/settings');
    await waitFor(() => {
      expect(document.documentElement.dataset['themeMode']).toBe('night-blue');
    });
    expect(document.documentElement.dataset['theme']).toBe('light');
    expect(themeLink()).not.toBeNull();
    expect(screen.getByRole('combobox', { name: 'Theme' })).toHaveValue('night-blue');
  });

  it('falls back to the default when the stored theme is gone', async () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, 'night-blue');
    themesServer([], 'nullradix');
    renderApp('/settings');
    await waitFor(() => {
      expect(document.documentElement.dataset['themeMode']).toBe('nullradix');
    });
    expect(document.documentElement.dataset['theme']).toBe('dark');
    expect(themeLink()).toBeNull();
  });

  it('keeps the stylesheet of the stored look while the list is loading', () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, 'night-blue');
    window.localStorage.setItem(
      INSTALLED_STORAGE_KEY,
      JSON.stringify({
        id: 'night-blue',
        scheme: 'dark',
        styleUrl: '/api/v1/themes/night-blue/files/theme.css?v=3',
        startColor: null,
      }),
    );
    fakeApi({ 'GET /api/v1/themes': () => new Promise<Response>(() => undefined) });
    renderApp('/settings');
    expect(themeLink()?.getAttribute('href')).toBe('/api/v1/themes/night-blue/files/theme.css?v=3');
    expect(document.documentElement.dataset['themeMode']).toBe('night-blue');
  });

  it('returns to the daemon choice when choosing fails', async () => {
    const api = themesServer([theme()]);
    api.set('PUT /api/v1/settings/theme', () => problem(404, 'theme-not-found', 'not installed'));
    renderApp('/settings');
    const select = await screen.findByRole('combobox', { name: 'Theme' });
    await screen.findByRole('option', { name: 'Night Blue' });
    await userEvent.selectOptions(select, 'night-blue');
    await waitFor(() => {
      expect(select).toHaveValue('nullradix');
    });
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe('nullradix');
  });

  it('offers a code switch only for themes that have code, with a warning', async () => {
    themesServer([
      theme({ hasCode: true, codeUrl: '/api/v1/themes/night-blue/files/main.js?v=1' }),
      theme({ id: 'plain-blue', name: 'Plain Blue' }),
    ]);
    renderApp('/settings');
    const code = await screen.findByRole('switch', { name: 'Run the code of Night Blue' });
    expect(code).not.toBeChecked();
    expect(screen.queryByRole('switch', { name: /Plain Blue/ })).not.toBeInTheDocument();
    expect(screen.getAllByText(/Theme code runs in NULLRADIX with full rights/)).toHaveLength(1);
  });

  it('turns the code of a theme on for the version it shows and back off', async () => {
    const api = themesServer([theme({ hasCode: true, version: 4 })]);
    renderApp('/settings');
    const code = await screen.findByRole('switch', { name: 'Run the code of Night Blue' });
    await userEvent.click(code);
    await waitFor(() => {
      expect(code).toBeChecked();
    });
    const [on] = api.requests('PUT /api/v1/themes/night-blue/code');
    expect(await on?.clone().json()).toEqual({ enabled: true, version: 4 });

    await userEvent.click(code);
    await waitFor(() => {
      expect(code).not.toBeChecked();
    });
    const sent = await Promise.all(
      api.requests('PUT /api/v1/themes/night-blue/code').map((request) => request.clone().json()),
    );
    expect(sent).toEqual([
      { enabled: true, version: 4 },
      { enabled: false, version: 4 },
    ]);
  });

  it('reports a theme that changed in the meantime and keeps the switch as it was', async () => {
    const api = themesServer([theme({ hasCode: true })]);
    api.set('PUT /api/v1/themes/night-blue/code', () => problem(409, 'version-conflict', 'stale'));
    renderApp('/settings');
    const code = await screen.findByRole('switch', { name: 'Run the code of Night Blue' });
    await userEvent.click(code);
    expect(
      await screen.findByText('This theme changed in the meantime. The list is up to date now.'),
    ).toBeInTheDocument();
    expect(code).not.toBeChecked();
  });

  it('refreshes the list when the switch changes somewhere else', async () => {
    const api = themesServer([theme({ hasCode: true })]);
    renderApp('/settings');
    const code = await screen.findByRole('switch', { name: 'Run the code of Night Blue' });
    expect(code).not.toBeChecked();
    api.replace([theme({ hasCode: true, codeEnabled: true })]);
    FakeEventSource.latest().emit('change', { seq: 1, entity: 'setting' });
    await waitFor(() => {
      expect(code).toBeChecked();
    });
  });

  it('asks before installing a package with code and sends the confirmed request', async () => {
    const api = themesServer();
    api.set('POST /api/v1/themes', (request) => {
      if (new URL(request.url).searchParams.get('confirmed') === 'true') {
        return json(201, theme());
      }
      return confirmation({
        hasCode: true,
        claudeTheme: true,
        version: '1.2.0',
        domains: ['fonts.example.com'],
      });
    });
    renderApp('/settings');
    await screen.findByText(/No theme package is installed/);
    await userEvent.upload(
      screen.getByLabelText('Install a theme package'),
      new File(['zip'], 'night-blue.nullradix'),
    );
    const dialog = await screen.findByRole('alertdialog', {
      name: 'Install Night Blue 1.2.0?',
    });
    expect(within(dialog).getByText(/This package contains code/)).toBeInTheDocument();
    expect(within(dialog).getByText('fonts.example.com')).toBeInTheDocument();
    expect(within(dialog).getByText(/Claude Code theme/)).toBeInTheDocument();
    expect(within(dialog).getByText(/code switch of this theme starts off/)).toBeInTheDocument();
    expect(screen.queryByText('the package needs a confirmation')).not.toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole('button', { name: 'Install' }));
    await waitFor(() => {
      expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    });
    const calls = api.requests('POST /api/v1/themes');
    expect(calls.map((request) => new URL(request.url).search)).toEqual(['', '?confirmed=true']);
    expect(await calls[1]?.clone().text()).toBe('zip');
  });

  it.each([
    [
      'adds nothing',
      { code: false, claudeTheme: false, domains: [] },
      /switch of this theme stays/,
    ],
    [
      'adds a domain',
      { code: false, claudeTheme: false, domains: ['a.b.com'] },
      /switch of this theme turns off/,
    ],
  ])('says what an update that %s does to the code switch', async (_name, added, line) => {
    const api = themesServer();
    api.set('POST /api/v1/themes', () =>
      confirmation({ update: true, hasCode: true, domains: ['a.b.com'], added }),
    );
    renderApp('/settings');
    await screen.findByText(/No theme package is installed/);
    await userEvent.upload(
      screen.getByLabelText('Install a theme package'),
      new File(['zip'], 'night-blue.nullradix'),
    );
    const dialog = await screen.findByRole('alertdialog', { name: 'Update Night Blue 1.0.0?' });
    expect(within(dialog).getByText(line)).toBeInTheDocument();
  });

  it('closes the confirmation with Escape and installs nothing', async () => {
    const api = themesServer();
    api.set('POST /api/v1/themes', () =>
      confirmation({
        update: true,
        domains: ['a.b.com'],
        added: { code: false, claudeTheme: false, domains: ['a.b.com'] },
      }),
    );
    renderApp('/settings');
    await screen.findByText(/No theme package is installed/);
    await userEvent.upload(
      screen.getByLabelText('Install a theme package'),
      new File(['zip'], 'night-blue.nullradix'),
    );
    const dialog = await screen.findByRole('alertdialog', { name: 'Update Night Blue 1.0.0?' });
    expect(within(dialog).getByText('a.b.com (new)')).toBeInTheDocument();
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(api.requests('POST /api/v1/themes')).toHaveLength(1);
    expect(screen.getByText(/No theme package is installed/)).toBeInTheDocument();
  });

  it('does not install when Enter is pressed right after the dialog opens', async () => {
    const api = themesServer();
    api.set('POST /api/v1/themes', () => confirmation({ hasCode: true }));
    renderApp('/settings');
    await screen.findByText(/No theme package is installed/);
    await userEvent.upload(
      screen.getByLabelText('Install a theme package'),
      new File(['zip'], 'night-blue.nullradix'),
    );
    const dialog = await screen.findByRole('alertdialog', {
      name: 'Install Night Blue 1.0.0?',
    });
    expect(within(dialog).getByRole('button', { name: 'Cancel' })).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(api.requests('POST /api/v1/themes')).toHaveLength(1);
  });
});
