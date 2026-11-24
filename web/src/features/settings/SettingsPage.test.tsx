import { act, screen } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { THEME_STORAGE_KEY } from '../../shared/lib/theme';
import { mockMatchMedia } from '../../test/matchMedia';
import { renderApp } from '../../test/renderApp';

describe('SettingsPage', () => {
  beforeEach(() => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.reject(new TypeError('offline'))),
    );
  });

  it('applies and stores the selected theme', async () => {
    renderApp('/settings');
    const select = screen.getByRole('combobox', { name: 'Theme' });
    expect(select).toHaveValue('nullradix');
    expect(document.documentElement.dataset['theme']).toBe('dark');

    await userEvent.selectOptions(select, 'light');

    expect(document.documentElement.dataset['theme']).toBe('light');
    expect(document.documentElement.dataset['themeMode']).toBe('light');
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe('light');
  });

  it('follows the system scheme in system mode', () => {
    const media = mockMatchMedia(false);
    window.localStorage.setItem(THEME_STORAGE_KEY, 'system');
    renderApp('/settings');
    expect(screen.getByRole('combobox', { name: 'Theme' })).toHaveValue('system');
    expect(document.documentElement.dataset['theme']).toBe('light');

    act(() => {
      media.setMatches(true);
    });

    expect(document.documentElement.dataset['theme']).toBe('dark');
  });
});
