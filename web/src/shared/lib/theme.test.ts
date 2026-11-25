import { describe, expect, it } from 'vitest';
import {
  DEFAULT_THEME_MODE,
  INSTALLED_STORAGE_KEY,
  THEME_LINK_ID,
  applyInstalledTheme,
  isThemeChoice,
  readInstalledLook,
  readThemeChoice,
  removeThemeStylesheet,
  saveInstalledLook,
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

describe('theme choices', () => {
  it('accepts built-in modes and ids shaped like installed themes', () => {
    expect(isThemeChoice('light')).toBe(true);
    expect(isThemeChoice('night-blue')).toBe(true);
    for (const bad of ['', 'a', 'Night', 'a b', '../x', 'x'.repeat(41), 7, null]) {
      expect(isThemeChoice(bad)).toBe(false);
    }
  });

  it('reads a stored choice or falls back to the default', () => {
    expect(readThemeChoice(storageWith('night-blue'))).toBe('night-blue');
    expect(readThemeChoice(storageWith('Bad Id'))).toBe(DEFAULT_THEME_MODE);
    expect(readThemeChoice(storageWith(null))).toBe(DEFAULT_THEME_MODE);
  });
});

describe('installed looks', () => {
  const look = {
    id: 'night-blue',
    scheme: 'dark' as const,
    styleUrl: '/api/v1/themes/night-blue/files/theme.css?v=1',
    startColor: null,
  };

  it('round-trips through storage for the same id only', () => {
    saveInstalledLook(window.localStorage, look);
    expect(readInstalledLook(window.localStorage, 'night-blue')).toEqual(look);
    expect(readInstalledLook(window.localStorage, 'other')).toBeNull();
    saveInstalledLook(window.localStorage, null);
    expect(window.localStorage.getItem(INSTALLED_STORAGE_KEY)).toBeNull();
  });

  it('ignores damaged stored looks', () => {
    for (const text of ['{', 'null', '[]', '{"id":"night-blue","scheme":"blue","styleUrl":"x"}']) {
      window.localStorage.setItem(INSTALLED_STORAGE_KEY, text);
      expect(readInstalledLook(window.localStorage, 'night-blue')).toBeNull();
    }
  });

  it('shows the scheme and replaces the stylesheet in place', () => {
    const root = document.documentElement;
    applyInstalledTheme(root, look);
    applyInstalledTheme(root, look);
    expect(document.querySelectorAll(`#${THEME_LINK_ID}`)).toHaveLength(1);
    applyInstalledTheme(root, { ...look, styleUrl: '/other.css' });
    expect(document.getElementById(THEME_LINK_ID)?.getAttribute('href')).toBe('/other.css');
    expect(root.dataset['theme']).toBe('dark');
    expect(root.dataset['themeMode']).toBe('night-blue');
    removeThemeStylesheet();
    expect(document.getElementById(THEME_LINK_ID)).toBeNull();
  });
});
