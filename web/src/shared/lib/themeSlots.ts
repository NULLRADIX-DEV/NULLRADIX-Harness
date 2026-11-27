import type { ThemeDefinition } from '@nullradix/theme-sdk';
import type { Theme } from '../query/themes';
import { getTheme } from './themeRegistry';

// The registered theme that counts now: the shown one, and only while its code switch is on.
export function activeThemeDefinition(
  mode: string,
  themes: readonly Theme[] | undefined,
): ThemeDefinition | undefined {
  const definition = getTheme(mode);
  if (!definition || themes === undefined) return definition;
  return themes.find((theme) => theme.id === mode)?.codeEnabled === true ? definition : undefined;
}
