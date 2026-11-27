import { screen, waitFor } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import { THEME_STORAGE_KEY, removeThemeStylesheet } from '../../shared/lib/theme';
import type { Theme, ThemeSettings } from '../../shared/query/themes';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';

const SETTINGS_ROUTE = '/api/v1/themes/night-blue/settings';

function theme(overrides: Partial<Theme> = {}): Theme {
  return {
    id: 'night-blue',
    name: 'Night Blue',
    packageVersion: '1.2.0',
    author: 'Ada',
    sdkVersion: '^1.3',
    colorScheme: 'dark',
    startColor: null,
    previewUrl: null,
    styleUrl: '/api/v1/themes/night-blue/files/theme.css?v=1',
    slots: [],
    network: [],
    settings: [
      { type: 'boolean', key: 'dim', label: 'Dim the sky', hint: 'Less light', default: true },
      {
        type: 'select',
        key: 'mode',
        label: 'Mode',
        default: 'calm',
        options: [
          { value: 'calm', label: 'Calm' },
          { value: 'bold', label: 'Bold' },
        ],
      },
    ],
    claudeTheme: false,
    hasCode: false,
    codeEnabled: false,
    installedAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

// In-memory daemon: one installed theme, its chosen state and the saved settings.
function server(options: { theme?: Theme; active?: string; saved?: ThemeSettings } = {}) {
  let saved: ThemeSettings = options.saved ?? { values: {}, version: 0 };
  const installed = options.theme ?? theme();
  const api = fakeApi({
    'GET /api/v1/themes': () =>
      json(200, { items: [installed], active: options.active ?? 'night-blue' }),
    'PUT /api/v1/settings/theme': async (request) => json(200, await request.json()),
    [`GET ${SETTINGS_ROUTE}`]: () => json(200, saved),
    [`PUT ${SETTINGS_ROUTE}`]: async (request) => {
      const body = (await request.json()) as ThemeSettings;
      if (body.version !== saved.version) return problem(409, 'version-conflict', 'stale');
      saved = { values: body.values, version: saved.version + 1 };
      return json(200, saved);
    },
    [`DELETE ${SETTINGS_ROUTE}`]: (request) => {
      const version = Number(new URL(request.url).searchParams.get('version'));
      if (version !== saved.version) return problem(409, 'version-conflict', 'stale');
      saved = { values: {}, version: 0 };
      return new Response(null, { status: 204 });
    },
  });
  return api;
}

afterEach(() => {
  removeThemeStylesheet();
  delete document.documentElement.dataset['theme'];
  delete document.documentElement.dataset['themeMode'];
});

describe('ThemeSettingsPanel', () => {
  it('is not shown for a built-in theme', async () => {
    const api = server({ active: 'light' });
    renderApp('/settings');
    await screen.findByRole('combobox', { name: 'Theme' });
    await waitFor(() => {
      expect(api.requests('GET /api/v1/themes').length).toBeGreaterThan(0);
    });
    expect(screen.queryByRole('heading', { name: /Settings of/ })).not.toBeInTheDocument();
    expect(api.requests(`GET ${SETTINGS_ROUTE}`)).toHaveLength(0);
  });

  it('is not shown for a theme that declares no settings', async () => {
    server({ theme: theme({ settings: [] }) });
    renderApp('/settings');
    await screen.findByRole('heading', { name: 'Installed themes' });
    expect(screen.queryByRole('heading', { name: /Settings of/ })).not.toBeInTheDocument();
  });

  it('shows the defaults of the manifest and then the saved values', async () => {
    server({ saved: { values: { dim: false, mode: 'bold' }, version: 3 } });
    renderApp('/settings');
    const dim = await screen.findByRole('checkbox', { name: 'Dim the sky' });
    await waitFor(() => {
      expect(dim).not.toBeChecked();
    });
    expect(screen.getByRole('combobox', { name: 'Mode' })).toHaveValue('bold');
    expect(screen.getByText('Less light')).toBeInTheDocument();
  });

  it('shows the defaults while nothing is saved and keeps reset disabled', async () => {
    server();
    renderApp('/settings');
    const dim = await screen.findByRole('checkbox', { name: 'Dim the sky' });
    await waitFor(() => {
      expect(dim).toBeEnabled();
    });
    expect(dim).toBeChecked();
    expect(screen.getByRole('combobox', { name: 'Mode' })).toHaveValue('calm');
    expect(screen.getByRole('button', { name: 'Reset to defaults' })).toBeDisabled();
  });

  it('saves only the values the person set, each change on top of the last', async () => {
    const api = server();
    renderApp('/settings');
    const dim = await screen.findByRole('checkbox', { name: 'Dim the sky' });
    await waitFor(() => {
      expect(dim).toBeEnabled();
    });
    await userEvent.click(dim);
    await waitFor(() => {
      expect(dim).not.toBeChecked();
    });
    await waitFor(() => {
      expect(dim).toBeEnabled();
    });
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Mode' }), 'bold');
    await waitFor(() => {
      expect(api.requests(`PUT ${SETTINGS_ROUTE}`)).toHaveLength(2);
    });
    const [first, second] = api.requests(`PUT ${SETTINGS_ROUTE}`);
    expect(await first?.clone().json()).toEqual({
      values: { dim: false },
      version: 0,
    });
    expect(await second?.clone().json()).toEqual({
      values: { dim: false, mode: 'bold' },
      version: 1,
    });
  });

  it('resets to the defaults with the saved version', async () => {
    const api = server({ saved: { values: { dim: false, mode: 'bold' }, version: 2 } });
    renderApp('/settings');
    const reset = await screen.findByRole('button', { name: 'Reset to defaults' });
    await waitFor(() => {
      expect(reset).toBeEnabled();
    });
    await userEvent.click(reset);
    await waitFor(() => {
      expect(screen.getByRole('checkbox', { name: 'Dim the sky' })).toBeChecked();
    });
    expect(screen.getByRole('combobox', { name: 'Mode' })).toHaveValue('calm');
    const [request] = api.requests(`DELETE ${SETTINGS_ROUTE}`);
    expect(new URL(request?.url ?? '').searchParams.get('version')).toBe('2');
    expect(screen.getByRole('button', { name: 'Reset to defaults' })).toBeDisabled();
  });

  it('shows a refused key at its own setting', async () => {
    const api = server();
    api.set(
      `PUT ${SETTINGS_ROUTE}`,
      () =>
        new Response(
          JSON.stringify({
            type: 'about:blank',
            title: 'Error',
            status: 422,
            code: 'theme-settings-invalid',
            detail: 'mode: the value is none of the options',
            errors: [{ field: 'mode', message: 'the value is none of the options' }],
          }),
          { status: 422, headers: { 'content-type': 'application/problem+json' } },
        ),
    );
    renderApp('/settings');
    const dim = await screen.findByRole('checkbox', { name: 'Dim the sky' });
    await waitFor(() => {
      expect(dim).toBeEnabled();
    });
    await userEvent.click(dim);
    const alert = await screen.findByText('the value is none of the options');
    const mode = screen.getByRole('combobox', { name: 'Mode' });
    expect(mode.closest('.theme-setting')).toContainElement(alert);
    expect(dim.closest('.theme-setting')).not.toContainElement(alert);
  });

  it('tells about a stale version and shows the current values again', async () => {
    const api = server();
    renderApp('/settings');
    const dim = await screen.findByRole('checkbox', { name: 'Dim the sky' });
    await waitFor(() => {
      expect(dim).toBeEnabled();
    });
    api.set(`PUT ${SETTINGS_ROUTE}`, () => problem(409, 'version-conflict', 'stale'));
    api.set(`GET ${SETTINGS_ROUTE}`, () => json(200, { values: { dim: false }, version: 4 }));
    await userEvent.click(dim);
    expect(
      await screen.findByText('These settings changed in the meantime. They are up to date now.'),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByRole('checkbox', { name: 'Dim the sky' })).not.toBeChecked();
    });
  });

  it('follows the choice of the theme', async () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, 'night-blue');
    server({ active: 'night-blue' });
    renderApp('/settings');
    expect(
      await screen.findByRole('heading', { name: 'Settings of Night Blue' }),
    ).toBeInTheDocument();
    await userEvent.selectOptions(await screen.findByRole('combobox', { name: 'Theme' }), 'light');
    await waitFor(() => {
      expect(screen.queryByRole('heading', { name: /Settings of/ })).not.toBeInTheDocument();
    });
  });
});
