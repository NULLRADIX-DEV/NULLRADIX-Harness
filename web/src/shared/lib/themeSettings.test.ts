import { describe, expect, it } from 'vitest';
import type { components } from '../api/schema';
import { resolveThemeSettings } from './themeSettings';

const specs: components['schemas']['ThemeSettingDto'][] = [
  { type: 'boolean', key: 'sounds', label: 'Sounds', default: true },
  {
    type: 'select',
    key: 'style',
    label: 'Style',
    default: 'pixel',
    options: [
      { value: 'pixel', label: 'Pixel' },
      { value: 'smooth', label: 'Smooth' },
    ],
  },
];

describe('resolveThemeSettings', () => {
  it('uses the defaults while nothing is saved', () => {
    expect(resolveThemeSettings(specs, {})).toEqual({ sounds: true, style: 'pixel' });
  });

  it('takes saved values that fit their setting', () => {
    expect(resolveThemeSettings(specs, { sounds: false, style: 'smooth' })).toEqual({
      sounds: false,
      style: 'smooth',
    });
  });

  it('falls back to the default for a wrong type or an unknown option and drops unknown keys', () => {
    const stored = { sounds: 'yes', style: 'checked', other: true };
    expect(resolveThemeSettings(specs, stored)).toEqual({ sounds: true, style: 'pixel' });
    expect(resolveThemeSettings(specs, { style: true })).toEqual({ sounds: true, style: 'pixel' });
  });

  it('has no values for a theme without settings', () => {
    expect(resolveThemeSettings([], { sounds: false })).toEqual({});
  });
});
