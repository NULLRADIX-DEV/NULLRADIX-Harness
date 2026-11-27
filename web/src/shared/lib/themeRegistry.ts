import type { ThemeDefinition } from '@nullradix/theme-sdk';

const themes = new Map<string, ThemeDefinition>();

// Registers a theme; one with the same id is replaced.
export function registerTheme(definition: ThemeDefinition): void {
  themes.set(definition.id, definition);
}

// Takes a theme out again, for example after it was uninstalled.
export function unregisterTheme(id: string): void {
  themes.delete(id);
}

export function getTheme(id: string): ThemeDefinition | undefined {
  return themes.get(id);
}
