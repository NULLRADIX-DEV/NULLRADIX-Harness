import type { QueryKey } from '@tanstack/react-query';

// Entity name from the change feed to the query key prefixes that cache it.
export type ChangeRegistry = Readonly<Record<string, readonly QueryKey[]>>;

export function defineChangeRegistry(...parts: ChangeRegistry[]): ChangeRegistry {
  const merged: Record<string, QueryKey[]> = {};
  for (const part of parts) {
    for (const [entity, keys] of Object.entries(part)) {
      merged[entity] = [...(merged[entity] ?? []), ...keys];
    }
  }
  return merged;
}
