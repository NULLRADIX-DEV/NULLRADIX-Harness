export const THEME_MODES = ['nullradix', 'light', 'dark', 'system'] as const;

export type ThemeMode = (typeof THEME_MODES)[number];

export type ResolvedTheme = 'light' | 'dark';

export const DEFAULT_THEME_MODE: ThemeMode = 'nullradix';

// Also read by the inline script in index.html before the first paint.
export const THEME_STORAGE_KEY = 'nullradix-theme';

export const DARK_SCHEME_QUERY = '(prefers-color-scheme: dark)';

export const THEME_LABELS: Readonly<Record<ThemeMode, string>> = {
  nullradix: 'NULLRADIX',
  light: 'Light',
  dark: 'Dark',
  system: 'System',
};

export function isThemeMode(value: unknown): value is ThemeMode {
  return typeof value === 'string' && (THEME_MODES as readonly string[]).includes(value);
}

export function resolveTheme(mode: ThemeMode, prefersDark: boolean): ResolvedTheme {
  switch (mode) {
    case 'nullradix':
    case 'dark':
      return 'dark';
    case 'light':
      return 'light';
    case 'system':
      return prefersDark ? 'dark' : 'light';
  }
}

export function systemPrefersDark(): boolean {
  return window.matchMedia(DARK_SCHEME_QUERY).matches;
}

export function readThemeMode(storage: Pick<Storage, 'getItem'>): ThemeMode {
  try {
    const stored = storage.getItem(THEME_STORAGE_KEY);
    return isThemeMode(stored) ? stored : DEFAULT_THEME_MODE;
  } catch {
    return DEFAULT_THEME_MODE;
  }
}

export function saveThemeMode(storage: Pick<Storage, 'setItem'>, mode: ThemeMode): void {
  try {
    storage.setItem(THEME_STORAGE_KEY, mode);
  } catch {
    // Blocked storage only loses the early start in the right scheme.
  }
}

export function applyTheme(root: HTMLElement, mode: ThemeMode, prefersDark: boolean): void {
  root.dataset['theme'] = resolveTheme(mode, prefersDark);
  root.dataset['themeMode'] = mode;
}
