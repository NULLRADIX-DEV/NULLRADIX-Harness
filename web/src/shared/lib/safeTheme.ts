// URL parameter of safe mode: no theme code and no theme CSS, only light or dark.
export const SAFE_THEME_PARAM = 'safe-theme';

// Is safe mode on? `?safe-theme` counts with a value too and next to other parameters.
export function isSafeTheme(search: string = window.location.search): boolean {
  return new URLSearchParams(search).has(SAFE_THEME_PARAM);
}

// The same address with `?safe-theme`; path, hash and other parameters stay.
export function safeThemeUrl(href: string): string {
  const url = new URL(href);
  if (!url.searchParams.has(SAFE_THEME_PARAM)) {
    url.search = url.search ? `${url.search}&${SAFE_THEME_PARAM}` : `?${SAFE_THEME_PARAM}`;
  }
  return url.toString();
}

// The same address without `safe-theme`.
export function normalThemeUrl(href: string): string {
  const url = new URL(href);
  url.searchParams.delete(SAFE_THEME_PARAM);
  return url.toString();
}
