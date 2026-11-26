import type { components } from '../api/schema';

type Spec = components['schemas']['ThemeSettingDto'];
export type ThemeSettingValue = components['schemas']['ThemeSettingValueDto'];

// One value per declared key: the saved one when it fits, else the default.
export function resolveThemeSettings(
  specs: readonly Spec[],
  stored: Readonly<Record<string, ThemeSettingValue>>,
): Record<string, ThemeSettingValue> {
  const values: Record<string, ThemeSettingValue> = {};
  for (const spec of specs) {
    const value = stored[spec.key];
    if (spec.type === 'boolean') {
      values[spec.key] = typeof value === 'boolean' ? value : spec.default;
    } else {
      const known = typeof value === 'string' && spec.options.some((o) => o.value === value);
      values[spec.key] = known ? value : spec.default;
    }
  }
  return values;
}
