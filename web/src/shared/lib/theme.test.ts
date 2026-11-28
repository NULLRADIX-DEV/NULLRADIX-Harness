import { afterEach, describe, expect, it } from 'vitest';
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

  it('shows the scheme and swaps the stylesheet', () => {
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

describe('theme stylesheet order', () => {
  const look = {
    id: 'night-blue',
    scheme: 'dark' as const,
    styleUrl: '/api/v1/themes/night-blue/files/theme.css?v=1',
    startColor: null,
  };

  afterEach(() => {
    document.head.replaceChildren();
  });

  function addLink(href: string, id?: string): HTMLLinkElement {
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = href;
    if (id) link.id = id;
    document.head.appendChild(link);
    return link;
  }

  // the head as boot-theme.js leaves it: the theme link comes before the built app stylesheet
  function bootHead(styleUrl: string): HTMLLinkElement {
    addLink('/boot-start.css');
    const boot = addLink(styleUrl, THEME_LINK_ID);
    addLink('/assets/index-abc123.css');
    return boot;
  }

  function hrefs(): (string | null)[] {
    return [...document.head.querySelectorAll('link')].map((link) => link.getAttribute('href'));
  }

  it('moves an early link with the same href after the app stylesheet', () => {
    const boot = bootHead(look.styleUrl);
    applyInstalledTheme(document.documentElement, look);
    const link = document.getElementById(THEME_LINK_ID);
    expect(link).not.toBe(boot);
    expect(document.head.lastElementChild).toBe(link);
    expect(link?.getAttribute('href')).toBe(look.styleUrl);
    // the early link keeps showing the theme until the placed one has loaded
    expect(boot.isConnected).toBe(true);
    link?.dispatchEvent(new Event('load'));
    expect(boot.isConnected).toBe(false);
    expect(hrefs()).toEqual(['/boot-start.css', '/assets/index-abc123.css', look.styleUrl]);
  });

  it('drops the early link also when the placed one fails to load', () => {
    const boot = bootHead(look.styleUrl);
    applyInstalledTheme(document.documentElement, look);
    document.getElementById(THEME_LINK_ID)?.dispatchEvent(new Event('error'));
    expect(boot.isConnected).toBe(false);
  });

  it('leaves a link that already comes after the app stylesheet', () => {
    addLink('/assets/index-abc123.css');
    applyInstalledTheme(document.documentElement, look);
    const link = document.getElementById(THEME_LINK_ID);
    applyInstalledTheme(document.documentElement, look);
    expect(document.getElementById(THEME_LINK_ID)).toBe(link);
    expect(document.head.querySelectorAll('link')).toHaveLength(2);
  });

  it('puts the link of a newly chosen theme after the app stylesheet', () => {
    bootHead(look.styleUrl);
    const next = { ...look, id: 'synthwave', styleUrl: '/api/v1/themes/synthwave/theme.css' };
    applyInstalledTheme(document.documentElement, next);
    expect(hrefs()).toEqual(['/boot-start.css', '/assets/index-abc123.css', next.styleUrl]);
    expect(document.documentElement.dataset['themeMode']).toBe('synthwave');
  });

  it('keeps the snippets last', () => {
    bootHead(look.styleUrl);
    const snippets = addLink('/api/v1/snippets/active.css?v=a1.1');
    snippets.dataset['snippets'] = '';
    const next = { ...look, styleUrl: '/other.css' };
    applyInstalledTheme(document.documentElement, next);
    expect(hrefs()).toEqual([
      '/boot-start.css',
      '/assets/index-abc123.css',
      '/other.css',
      '/api/v1/snippets/active.css?v=a1.1',
    ]);
  });

  it('removes the early link together with the placed one', () => {
    bootHead(look.styleUrl);
    applyInstalledTheme(document.documentElement, look);
    removeThemeStylesheet();
    expect(hrefs()).toEqual(['/boot-start.css', '/assets/index-abc123.css']);
  });
});
