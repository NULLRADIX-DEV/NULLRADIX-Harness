import type { ThemeDefinition } from '@nullradix/theme-sdk';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Theme, ThemeList } from '../query/themes';
import { loadThemeModule, type ThemeModuleDeps } from './loadThemeModule';
import { THEME_STORAGE_KEY } from './theme';
import { getThemeNotice, setThemeNotice } from './themeNotice';
import { getTheme, unregisterTheme } from './themeRegistry';

function theme(overrides: Partial<Theme> = {}): Theme {
  return {
    id: 'ocean',
    name: 'Ocean',
    packageVersion: '1.0.0',
    author: 'Ada',
    sdkVersion: '^1.3',
    colorScheme: 'dark',
    startColor: null,
    previewUrl: null,
    styleUrl: '/api/v1/themes/ocean/files/theme.css?v=1',
    slots: ['Progress'],
    network: [],
    settings: [],
    claudeTheme: false,
    hasCode: true,
    codeEnabled: true,
    codeUrl: '/api/v1/themes/ocean/files/main.js?v=1',
    installedAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

function list(items: Theme[] = [theme()], active = 'ocean'): ThemeList {
  return { items, active };
}

const definition: ThemeDefinition = { id: 'ocean', slots: {} };

function deps(overrides: Partial<ThemeModuleDeps> = {}) {
  return {
    importModule: vi.fn(() => Promise.resolve({ default: definition })),
    fetchThemes: vi.fn(() => Promise.resolve(list())),
    sdkVersion: '1.3.0',
    search: '',
    timeoutMs: 50,
    ...overrides,
  };
}

beforeEach(() => {
  window.localStorage.setItem(THEME_STORAGE_KEY, 'ocean');
  vi.spyOn(console, 'warn').mockImplementation(() => undefined);
});

afterEach(() => {
  unregisterTheme('ocean');
  setThemeNotice(undefined);
});

describe('loadThemeModule', () => {
  it('imports main.js of the chosen theme and registers its definition', async () => {
    const d = deps();
    const result = await loadThemeModule(d);
    expect(result).toEqual({ status: 'loaded', themeId: 'ocean' });
    expect(d.importModule).toHaveBeenCalledWith('/api/v1/themes/ocean/files/main.js?v=1');
    expect(getTheme('ocean')).toEqual(definition);
    expect(getThemeNotice()).toBeUndefined();
  });

  it('registers a copy of the definition that it read once', async () => {
    const Button = () => null;
    let reads = 0;
    const module = {
      id: 'ocean',
      get slots() {
        reads += 1;
        return { Button };
      },
    };
    const result = await loadThemeModule(
      deps({ importModule: vi.fn(() => Promise.resolve({ default: module })) }),
    );
    expect(result.status).toBe('loaded');
    expect(reads).toBe(1);
    expect(getTheme('ocean')?.slots).toEqual({ Button });
    expect(getTheme('ocean')).not.toBe(module);
  });

  it('follows the theme the daemon has chosen, not the stored hint', async () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, 'light');
    const d = deps();
    expect((await loadThemeModule(d)).status).toBe('loaded');
    expect(d.importModule).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['the switch is off', { codeEnabled: false }],
    ['the theme has no code', { hasCode: false, codeEnabled: false, codeUrl: null }],
    ['there is no address', { codeUrl: null }],
  ])('runs nothing when %s', async (_name, overrides) => {
    const d = deps({ fetchThemes: vi.fn(() => Promise.resolve(list([theme(overrides)]))) });
    expect(await loadThemeModule(d)).toEqual({ status: 'skipped', themeId: 'ocean' });
    expect(d.importModule).not.toHaveBeenCalled();
    expect(getTheme('ocean')).toBeUndefined();
    expect(getThemeNotice()).toBeUndefined();
  });

  it.each(['nullradix', 'system', 'light', 'dark'])(
    'runs nothing while the built-in theme %s is chosen',
    async (active) => {
      const d = deps({ fetchThemes: vi.fn(() => Promise.resolve(list([theme()], active))) });
      expect((await loadThemeModule(d)).status).toBe('skipped');
      expect(d.importModule).not.toHaveBeenCalled();
    },
  );

  it('runs nothing for a theme that is no longer installed', async () => {
    const d = deps({ fetchThemes: vi.fn(() => Promise.resolve(list([], 'ocean'))) });
    expect((await loadThemeModule(d)).status).toBe('skipped');
    expect(d.importModule).not.toHaveBeenCalled();
  });

  it('skips a module whose sdk range does not fit and names both versions', async () => {
    const d = deps({
      fetchThemes: vi.fn(() => Promise.resolve(list([theme({ sdkVersion: '^9' })]))),
    });
    expect((await loadThemeModule(d)).status).toBe('incompatible');
    expect(d.importModule).not.toHaveBeenCalled();
    expect(getTheme('ocean')).toBeUndefined();
    expect(getThemeNotice()).toMatchObject({
      kind: 'error',
      message: expect.stringContaining('SDK 1.3.0, needs ^9') as string,
    });
  });

  it('checks the range against the SDK version of the build by default', async () => {
    const { SDK_VERSION } = await import('@nullradix/theme-sdk');
    const fits = deps({
      fetchThemes: vi.fn(() => Promise.resolve(list([theme({ sdkVersion: `=${SDK_VERSION}` })]))),
    });
    Reflect.deleteProperty(fits, 'sdkVersion');
    expect((await loadThemeModule(fits)).status).toBe('loaded');
  });

  it('falls back to the default when the import fails', async () => {
    const d = deps({ importModule: vi.fn(() => Promise.reject(new Error('broken'))) });
    expect((await loadThemeModule(d)).status).toBe('failed');
    expect(getTheme('ocean')).toBeUndefined();
    expect(getThemeNotice()?.message).toContain('could not be loaded (Error: broken)');
  });

  it('says so when the theme list is unreadable while an installed theme is chosen', async () => {
    const d = deps({ fetchThemes: vi.fn(() => Promise.reject(new Error('HTTP 500'))) });
    expect((await loadThemeModule(d)).status).toBe('failed');
    expect(d.importModule).not.toHaveBeenCalled();
    expect(getThemeNotice()?.message).toContain('the theme list is unreadable: HTTP 500');
  });

  it('stays quiet when the theme list is unreadable and a built-in theme is chosen', async () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, 'dark');
    const d = deps({ fetchThemes: vi.fn(() => Promise.reject(new Error('HTTP 401'))) });
    expect(await loadThemeModule(d)).toEqual({ status: 'skipped' });
    expect(getThemeNotice()).toBeUndefined();
  });

  it('gives up after the time limit and drops a module that arrives late', async () => {
    let arrive: (module: unknown) => void = () => undefined;
    const late = new Promise((resolve) => {
      arrive = resolve;
    });
    const d = deps({ importModule: vi.fn(() => late), timeoutMs: 20 });
    expect((await loadThemeModule(d)).status).toBe('timeout');
    expect(getThemeNotice()?.message).toContain('was not loaded in time');
    arrive({ default: definition });
    await late;
    expect(getTheme('ocean')).toBeUndefined();
  });

  it('counts the wait for the theme list into the time limit', async () => {
    const d = deps({ fetchThemes: vi.fn(() => new Promise<ThemeList>(() => undefined)) });
    expect((await loadThemeModule({ ...d, timeoutMs: 20 })).status).toBe('timeout');
    expect(d.importModule).not.toHaveBeenCalled();
  });

  it.each([
    ['without slots', { default: { id: 'ocean' } }],
    ['with another id', { default: { id: 'other', slots: {} } }],
    ['without a default', {}],
    ['that is no object', { default: 'ocean' }],
    ['that is null', null],
  ])('refuses an export %s', async (_name, module) => {
    const d = deps({ importModule: vi.fn(() => Promise.resolve(module)) });
    expect((await loadThemeModule(d)).status).toBe('invalid');
    expect(getTheme('ocean')).toBeUndefined();
    expect(getThemeNotice()?.message).toContain('no valid theme definition');
  });
});

describe('loadThemeModule in safe mode', () => {
  it('asks the daemon for nothing, imports nothing and says so', async () => {
    const d = deps({ search: '?safe-theme' });
    expect(await loadThemeModule(d)).toEqual({ status: 'skipped', themeId: 'ocean' });
    expect(d.fetchThemes).not.toHaveBeenCalled();
    expect(d.importModule).not.toHaveBeenCalled();
    expect(getThemeNotice()).toEqual({
      kind: 'info',
      message: 'Safe mode: code and CSS of the theme "ocean" are off.',
    });
  });

  it('stays quiet while a built-in theme is chosen', async () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, 'light');
    const d = deps({ search: '?x=1&safe-theme=1' });
    expect(await loadThemeModule(d)).toEqual({ status: 'skipped' });
    expect(d.fetchThemes).not.toHaveBeenCalled();
    expect(getThemeNotice()).toBeUndefined();
  });

  it('reads the flag from the address of the page by default', async () => {
    window.history.replaceState(null, '', '/board?safe-theme');
    try {
      const d = deps();
      Reflect.deleteProperty(d, 'search');
      expect((await loadThemeModule(d)).status).toBe('skipped');
      expect(d.importModule).not.toHaveBeenCalled();
    } finally {
      window.history.replaceState(null, '', '/');
    }
  });
});
