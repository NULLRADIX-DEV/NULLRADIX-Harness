import type { ThemeDefinition } from '@nullradix/theme-sdk';
import { afterEach, describe, expect, it } from 'vitest';
import { themeEntry } from '../../test/renderThemed';
import { activeThemeDefinition } from './themeSlots';
import { registerTheme, unregisterTheme } from './themeRegistry';

const ocean: ThemeDefinition = { id: 'ocean', slots: {} };

afterEach(() => {
  unregisterTheme('ocean');
});

describe('activeThemeDefinition', () => {
  it('has nothing for a theme that registered nothing', () => {
    expect(activeThemeDefinition('ocean', [themeEntry('ocean')])).toBeUndefined();
  });

  it('gives the registration of the shown theme while its code switch is on', () => {
    registerTheme(ocean);
    expect(activeThemeDefinition('ocean', [themeEntry('ocean')])).toEqual(ocean);
  });

  it('ignores the registration of a theme that is not shown', () => {
    registerTheme(ocean);
    expect(activeThemeDefinition('light', [themeEntry('ocean')])).toBeUndefined();
  });

  it('drops the registration once the code switch is off or the theme is gone', () => {
    registerTheme(ocean);
    expect(
      activeThemeDefinition('ocean', [themeEntry('ocean', { codeEnabled: false })]),
    ).toBeUndefined();
    expect(activeThemeDefinition('ocean', [themeEntry('forest')])).toBeUndefined();
  });

  it('trusts the registration while the theme list is not known yet', () => {
    registerTheme(ocean);
    expect(activeThemeDefinition('ocean', undefined)).toEqual(ocean);
  });
});
