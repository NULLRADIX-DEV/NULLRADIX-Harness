import { afterEach, describe, expect, it } from 'vitest';
import { isSafeTheme, normalThemeUrl, safeThemeUrl } from './safeTheme';

afterEach(() => {
  window.history.replaceState(null, '', '/');
});

describe('isSafeTheme', () => {
  it('finds the parameter in every form', () => {
    expect(isSafeTheme('?safe-theme')).toBe(true);
    expect(isSafeTheme('?safe-theme=1')).toBe(true);
    expect(isSafeTheme('?x=1&safe-theme')).toBe(true);
    expect(isSafeTheme('?x=1')).toBe(false);
    expect(isSafeTheme('')).toBe(false);
    expect(isSafeTheme('?safe-themes')).toBe(false);
  });

  it('reads the address of the page without an argument', () => {
    window.history.replaceState(null, '', '/board?a=2&safe-theme');
    expect(isSafeTheme()).toBe(true);
    window.history.replaceState(null, '', '/board');
    expect(isSafeTheme()).toBe(false);
  });
});

describe('safeThemeUrl and normalThemeUrl', () => {
  it('go there and back; path, hash and other parameters stay', () => {
    const safe = safeThemeUrl('http://localhost/board?a=2#x');
    expect(safe).toBe('http://localhost/board?a=2&safe-theme#x');
    expect(normalThemeUrl(safe)).toBe('http://localhost/board?a=2#x');
    expect(normalThemeUrl('http://localhost/board?safe-theme=1')).toBe('http://localhost/board');
  });

  it('adds the parameter once', () => {
    expect(safeThemeUrl('http://localhost/board')).toBe('http://localhost/board?safe-theme');
    expect(safeThemeUrl('http://localhost/board?safe-theme')).toBe(
      'http://localhost/board?safe-theme',
    );
  });
});
