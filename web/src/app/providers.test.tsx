import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { registerTheme, unregisterTheme } from '../shared/lib/themeRegistry';
import { themeKeys } from '../shared/query/themes';
import { fakeApi, json } from '../test/fakeApi';
import { mockMatchMedia } from '../test/matchMedia';
import { themeEntry } from '../test/renderThemed';
import { AppProviders } from './providers';
import { createQueryClient } from './queryClient';

const OCEAN = 'ocean';

function renderProviders() {
  registerTheme({ id: OCEAN, slots: { StartAnimation: () => <span>intro</span> } });
  const list = { items: [themeEntry(OCEAN)], active: OCEAN };
  fakeApi({ 'GET /api/v1/themes': () => json(200, list) });
  const queryClient = createQueryClient({ retry: false });
  queryClient.setQueryData(themeKeys.list(), list);
  render(
    <AppProviders queryClient={queryClient}>
      <span>app</span>
    </AppProviders>,
  );
}

function openAt(path: string, state: unknown) {
  window.history.replaceState(state, '', path);
}

beforeEach(() => {
  mockMatchMedia(false);
});

afterEach(() => {
  unregisterTheme(OCEAN);
  window.history.replaceState(null, '', '/');
});

describe('the start animation of a theme', () => {
  it('plays on a load into the app', () => {
    renderProviders();
    expect(screen.getByText('intro')).toBeInTheDocument();
  });

  it('is left out when the page loads on the settings shown in the built-in shell', () => {
    openAt('/settings', { usr: { builtinShell: true }, key: 'k', idx: 0 });
    renderProviders();
    expect(screen.getByText('app')).toBeInTheDocument();
    expect(screen.queryByText('intro')).not.toBeInTheDocument();
  });

  it('plays on the settings when the built-in shell was not asked for', () => {
    openAt('/settings', { usr: null, key: 'k', idx: 0 });
    renderProviders();
    expect(screen.getByText('intro')).toBeInTheDocument();
  });

  it('plays elsewhere even when the state asks for the built-in shell', () => {
    openAt('/inbox', { usr: { builtinShell: true }, key: 'k', idx: 0 });
    renderProviders();
    expect(screen.getByText('intro')).toBeInTheDocument();
  });
});
