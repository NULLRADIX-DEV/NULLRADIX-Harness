import type { ThemeDefinition } from '@nullradix/theme-sdk';
import { afterEach, expect, it } from 'vitest';
import { getTheme, registerTheme, unregisterTheme } from './themeRegistry';

const ocean: ThemeDefinition = { id: 'ocean', slots: {} };

afterEach(() => {
  unregisterTheme('ocean');
});

it('keeps what a module registered until it is taken out', () => {
  expect(getTheme('ocean')).toBeUndefined();
  registerTheme(ocean);
  expect(getTheme('ocean')).toBe(ocean);
  unregisterTheme('ocean');
  expect(getTheme('ocean')).toBeUndefined();
});

it('replaces a theme registered again under the same id', () => {
  const again: ThemeDefinition = { id: 'ocean', slots: {} };
  registerTheme(ocean);
  registerTheme(again);
  expect(getTheme('ocean')).toBe(again);
});
