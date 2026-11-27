import { QueryClientProvider } from '@tanstack/react-query';
import { render } from '@testing-library/react';
import type { ThemeDefinition } from '@nullradix/theme-sdk';
import type { ReactNode } from 'react';
import { afterEach } from 'vitest';
import { createQueryClient } from '../app/queryClient';
import { removeThemeStylesheet } from '../shared/lib/theme';
import { forgetThemeFailures } from '../shared/lib/themeFailure';
import { setThemeNotice } from '../shared/lib/themeNotice';
import { registerTheme, unregisterTheme } from '../shared/lib/themeRegistry';
import { themeKeys, type Theme, type ThemeList } from '../shared/query/themes';
import { ThemeProvider } from '../shared/ui/ThemeProvider';
import { fakeApi, json, type Handler } from './fakeApi';

export function themeEntry(id: string, overrides: Partial<Theme> = {}): Theme {
  return {
    id,
    name: id,
    packageVersion: '1.0.0',
    author: 'Ada',
    sdkVersion: '^1.3',
    colorScheme: 'dark',
    startColor: null,
    previewUrl: null,
    styleUrl: `/api/v1/themes/${id}/files/theme.css?v=1`,
    slots: [],
    network: [],
    settings: [],
    claudeTheme: false,
    hasCode: true,
    codeEnabled: true,
    codeUrl: `/api/v1/themes/${id}/files/main.js?v=1`,
    installedAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

const registered = new Set<string>();

afterEach(() => {
  for (const id of registered) unregisterTheme(id);
  registered.clear();
  forgetThemeFailures();
  setThemeNotice(undefined);
  removeThemeStylesheet();
  document.documentElement.removeAttribute('data-theme');
  document.documentElement.removeAttribute('data-theme-mode');
});

// Renders under the real ThemeProvider with a module registered at boot and the theme list cached.
export function renderThemed(
  ui: ReactNode,
  definition: ThemeDefinition,
  options: {
    codeEnabled?: boolean;
    entry?: Partial<Theme>;
    routes?: Record<string, Handler>;
  } = {},
) {
  registerTheme(definition);
  registered.add(definition.id);
  const list: ThemeList = {
    items: [
      themeEntry(definition.id, { codeEnabled: options.codeEnabled ?? true, ...options.entry }),
    ],
    active: definition.id,
  };
  const state = { list };
  fakeApi({
    ...options.routes,
    'GET /api/v1/themes': () => json(200, state.list),
    'PUT /api/v1/settings/theme': async (request) => {
      const body = (await request.json()) as { theme: string };
      state.list = { ...state.list, active: body.theme };
      return json(200, body);
    },
  });
  const queryClient = createQueryClient({ retry: false });
  queryClient.setQueryData(themeKeys.list(), list);
  const view = render(
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>{ui}</ThemeProvider>
    </QueryClientProvider>,
  );
  return { ...view, queryClient, state };
}
