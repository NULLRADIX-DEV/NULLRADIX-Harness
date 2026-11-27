import { afterEach, describe, expect, it } from 'vitest';
import script from '../../public/boot-theme.js?raw';
import { INSTALLED_STORAGE_KEY, THEME_LINK_ID, THEME_STORAGE_KEY } from '../shared/lib/theme';
import { mockMatchMedia } from './matchMedia';

const look = {
  id: 'night-blue',
  scheme: 'dark',
  styleUrl: '/api/v1/themes/night-blue/files/theme.css?v=1',
  startColor: '#101018',
};

// Runs the script as a classic script, like the browser does before the first paint.
function boot(): void {
  (0, eval)(script);
}

function installLook(): void {
  window.localStorage.setItem(THEME_STORAGE_KEY, 'night-blue');
  window.localStorage.setItem(INSTALLED_STORAGE_KEY, JSON.stringify(look));
}

function link(): Element | null {
  return document.getElementById(THEME_LINK_ID);
}

afterEach(() => {
  link()?.remove();
  delete document.documentElement.dataset['theme'];
  delete document.documentElement.dataset['themeMode'];
  document.documentElement.style.removeProperty('--start-bg');
  window.history.replaceState(null, '', '/');
});

describe('boot-theme.js', () => {
  it('shows the stored look of an installed theme with its stylesheet', () => {
    installLook();
    boot();
    expect(document.documentElement.dataset['theme']).toBe('dark');
    expect(document.documentElement.dataset['themeMode']).toBe('night-blue');
    expect(link()?.getAttribute('href')).toBe(look.styleUrl);
  });

  it('shows the default for an installed theme without a stored look', () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, 'night-blue');
    boot();
    expect(document.documentElement.dataset['themeMode']).toBe('nullradix');
    expect(link()).toBeNull();
  });

  it('shows an installed theme as light without its stylesheet in safe mode', () => {
    installLook();
    window.history.replaceState(null, '', '/board?x=1&safe-theme');
    boot();
    expect(document.documentElement.dataset['theme']).toBe('light');
    expect(document.documentElement.dataset['themeMode']).toBe('night-blue');
    expect(link()).toBeNull();
  });

  it('follows the system in safe mode', () => {
    installLook();
    mockMatchMedia(true);
    window.history.replaceState(null, '', '/?safe-theme=1');
    boot();
    expect(document.documentElement.dataset['theme']).toBe('dark');
    expect(link()).toBeNull();
  });

  it('leaves built-in themes alone in safe mode', () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, 'light');
    window.history.replaceState(null, '', '/?safe-theme');
    boot();
    expect(document.documentElement.dataset['theme']).toBe('light');
    expect(document.documentElement.dataset['themeMode']).toBe('light');
    window.localStorage.setItem(THEME_STORAGE_KEY, 'dark');
    boot();
    expect(document.documentElement.dataset['theme']).toBe('dark');
  });
});
