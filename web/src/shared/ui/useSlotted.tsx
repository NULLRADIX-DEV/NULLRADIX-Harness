import type { ThemeSlots } from '@nullradix/theme-sdk';
import { createElement, type ComponentType, type ReactNode } from 'react';
import { SlotBoundary } from './SlotBoundary';
import { useSlotTheme } from './useSlot';

// The props of the component a slot takes.
export type SlotProps<K extends keyof ThemeSlots> =
  NonNullable<ThemeSlots[K]> extends ComponentType<infer P> ? P : never;

// The theme component of a slot in a boundary, or the built-in node; `replaced` marks a theme one.
export function useSlotted<K extends keyof ThemeSlots>(
  slot: K,
  props: SlotProps<K>,
  standard: ReactNode,
): { node: ReactNode; replaced: boolean } {
  const theme = useSlotTheme();
  const Custom = theme?.slots[slot] as ComponentType<SlotProps<K>> | undefined;
  if (!theme || !Custom) return { node: standard, replaced: false };
  return {
    node: (
      <SlotBoundary fallback={standard} themeId={theme.id} where={`the slot ${slot}`}>
        {createElement(Custom as ComponentType<object>, props as object)}
      </SlotBoundary>
    ),
    replaced: true,
  };
}
