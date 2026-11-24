import { describe, expect, it } from 'vitest';
import {
  DEFAULT_THEME_MODE,
  THEME_STORAGE_KEY,
  applyTheme,
  isThemeMode,
  readThemeMode,
  resolveTheme,
  saveThemeMode,
} from './theme';

function storageWith(value: string | null): Pick<Storage, 'getItem'> {
  return { getItem: () => value };
}

describe('resolveTheme', () => {
  it('shows nullradix and dark as dark', () => {
    expect(resolveTheme('nullradix', false)).toBe('dark');
    expect(resolveTheme('dark', false)).toBe('dark');
  });

  it('shows light as light', () => {
    expect(resolveTheme('light', true)).toBe('light');
  });

  it('follows the system preference for system', () => {
    expect(resolveTheme('system', true)).toBe('dark');
    expect(resolveTheme('system', false)).toBe('light');
  });
});

describe('isThemeMode', () => {
  it('accepts only the built-in modes', () => {
    expect(isThemeMode('light')).toBe(true);
    expect(isThemeMode('neon')).toBe(false);
    expect(isThemeMode(null)).toBe(false);
  });
});

describe('readThemeMode', () => {
  it('defaults to nullradix', () => {
    expect(DEFAULT_THEME_MODE).toBe('nullradix');
    expect(readThemeMode(storageWith(null))).toBe('nullradix');
  });

  it('returns a stored mode', () => {
    expect(readThemeMode(storageWith('system'))).toBe('system');
  });

  it('ignores unknown values', () => {
    expect(readThemeMode(storageWith('neon'))).toBe('nullradix');
  });

  it('survives blocked storage', () => {
    const blocked = {
      getItem: (): string | null => {
        throw new Error('blocked');
      },
    };
    expect(readThemeMode(blocked)).toBe('nullradix');
  });
});

describe('saveThemeMode', () => {
  it('stores the mode under the shared key', () => {
    saveThemeMode(window.localStorage, 'light');
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe('light');
  });

  it('ignores blocked storage', () => {
    const blocked = {
      setItem: () => {
        throw new Error('blocked');
      },
    };
    expect(() => {
      saveThemeMode(blocked, 'dark');
    }).not.toThrow();
  });
});

describe('applyTheme', () => {
  it('sets the displayed scheme and the mode on the root', () => {
    const root = document.createElement('html');
    applyTheme(root, 'nullradix', false);
    expect(root.dataset['theme']).toBe('dark');
    expect(root.dataset['themeMode']).toBe('nullradix');
    applyTheme(root, 'system', false);
    expect(root.dataset['theme']).toBe('light');
    expect(root.dataset['themeMode']).toBe('system');
  });
});
