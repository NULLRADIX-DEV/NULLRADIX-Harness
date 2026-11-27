import type { ThemeDefinition, ThemeSlots } from '@nullradix/theme-sdk';

const themes = new Map<string, ThemeDefinition>();

// Own enumerable function values, read once, so later changes or getters change nothing.
function snapshot(definition: ThemeDefinition, id: string): ThemeDefinition {
  const { settingsSchema, mermaid } = definition;
  const slots = Object.fromEntries(
    Object.entries(definition.slots).filter(([, value]) => typeof value === 'function'),
  ) as ThemeSlots;
  return {
    id,
    slots,
    ...(settingsSchema !== undefined && { settingsSchema }),
    ...(typeof mermaid === 'function' && { mermaid }),
  };
}

// Registers a copy of a theme under `id`; one with the same id is replaced.
export function registerTheme(definition: ThemeDefinition, id = definition.id): void {
  themes.set(id, snapshot(definition, id));
}

// Takes a theme out again, for example after it was uninstalled.
export function unregisterTheme(id: string): void {
  themes.delete(id);
}

export function getTheme(id: string): ThemeDefinition | undefined {
  return themes.get(id);
}
