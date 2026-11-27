import type { ThemeDefinition, ThemeSlots } from '@nullradix/theme-sdk';
import { createContext, use } from 'react';

// The theme whose registration counts; ThemeProvider sets it, nothing means the built-in look.
export const SlotThemeContext = createContext<ThemeDefinition | undefined>(undefined);

export function useSlotTheme(): ThemeDefinition | undefined {
  return use(SlotThemeContext);
}

// The component the active theme registered for a slot; undefined keeps the built-in one.
export function useSlot<K extends keyof ThemeSlots>(slot: K): ThemeSlots[K] | undefined {
  return useSlotTheme()?.slots[slot];
}
