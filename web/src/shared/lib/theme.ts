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

export function saveThemeMode(storage: Pick<Storage, 'setItem'>, mode: string): void {
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

// An installed theme has an id like its folder; built-in ids fit the same shape or are short words.
export const THEME_ID_PATTERN = /^[a-z0-9-]{2,40}$/;

// Also read by the inline script in index.html: look of the installed theme, for the first paint.
export const INSTALLED_STORAGE_KEY = 'nullradix-theme-installed';

export const THEME_LINK_ID = 'nullradix-theme-css';

export interface InstalledLook {
  id: string;
  scheme: ResolvedTheme;
  styleUrl: string;
  startColor: string | null;
}

export function isThemeChoice(value: unknown): value is string {
  return typeof value === 'string' && (isThemeMode(value) || THEME_ID_PATTERN.test(value));
}

export function readThemeChoice(storage: Pick<Storage, 'getItem'>): string {
  try {
    const stored = storage.getItem(THEME_STORAGE_KEY);
    return isThemeChoice(stored) ? stored : DEFAULT_THEME_MODE;
  } catch {
    return DEFAULT_THEME_MODE;
  }
}

export function readInstalledLook(
  storage: Pick<Storage, 'getItem'>,
  id: string,
): InstalledLook | null {
  try {
    const parsed: unknown = JSON.parse(storage.getItem(INSTALLED_STORAGE_KEY) ?? 'null');
    if (typeof parsed !== 'object' || parsed === null) return null;
    const look = parsed as Partial<InstalledLook>;
    const valid =
      look.id === id &&
      (look.scheme === 'light' || look.scheme === 'dark') &&
      typeof look.styleUrl === 'string';
    return valid ? (parsed as InstalledLook) : null;
  } catch {
    return null;
  }
}

export function saveInstalledLook(
  storage: Pick<Storage, 'setItem' | 'removeItem'>,
  look: InstalledLook | null,
): void {
  try {
    if (look) storage.setItem(INSTALLED_STORAGE_KEY, JSON.stringify(look));
    else storage.removeItem(INSTALLED_STORAGE_KEY);
  } catch {
    // Blocked storage only loses the early start in the right look.
  }
}

// Shows an installed theme: its scheme, its mode and its stylesheet after the app styles.
export function applyInstalledTheme(root: HTMLElement, look: InstalledLook): void {
  root.dataset['theme'] = look.scheme;
  root.dataset['themeMode'] = look.id;
  const doc = root.ownerDocument;
  const existing = doc.getElementById(THEME_LINK_ID);
  if (existing instanceof HTMLLinkElement && existing.getAttribute('href') === look.styleUrl) {
    return;
  }
  const link = doc.createElement('link');
  link.id = THEME_LINK_ID;
  link.rel = 'stylesheet';
  link.href = look.styleUrl;
  if (existing) existing.replaceWith(link);
  else doc.head.appendChild(link);
}

export function removeThemeStylesheet(doc: Document = document): void {
  doc.getElementById(THEME_LINK_ID)?.remove();
}
