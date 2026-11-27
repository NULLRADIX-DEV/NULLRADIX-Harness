import type { ThemeDefinition, ThemeSettingsSchema, ThemeSlots } from '@nullradix/theme-sdk';

const themes = new Map<string, ThemeDefinition>();

// A schema is plain data: a copy keeps later changes out; one with code in it is dropped.
function copySchema(schema: unknown): ThemeSettingsSchema | undefined {
  if (typeof schema !== 'object' || schema === null) return undefined;
  try {
    return structuredClone(schema) as ThemeSettingsSchema;
  } catch {
    return undefined;
  }
}

// A theme module's default export as a plain copy, each property read once; undefined if no theme.
export function readTheme(value: unknown, id: string): ThemeDefinition | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const { id: found, slots, settingsSchema, mermaid } = value as Record<string, unknown>;
  if (found !== id || typeof slots !== 'object' || slots === null) return undefined;
  const components = Object.fromEntries(
    Object.entries(slots).filter(([, component]) => typeof component === 'function'),
  ) as ThemeSlots;
  const schema = copySchema(settingsSchema);
  return {
    id,
    slots: components,
    ...(schema !== undefined && { settingsSchema: schema }),
    ...(typeof mermaid === 'function' && {
      mermaid: mermaid as NonNullable<ThemeDefinition['mermaid']>,
    }),
  };
}

// Registers a theme, usually one from readTheme; one with the same id is replaced.
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
