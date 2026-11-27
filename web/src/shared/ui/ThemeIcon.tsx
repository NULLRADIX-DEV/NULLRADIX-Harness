import type { IconName } from '@nullradix/theme-sdk';
import type { ReactNode } from 'react';
import { SlotBoundary } from './SlotBoundary';
import { useSlotTheme } from './useSlot';

// A core icon that the slot Icon may replace; the children are the built-in one.
export function ThemeIcon({ name, children = null }: { name: IconName; children?: ReactNode }) {
  const theme = useSlotTheme();
  const Custom = theme?.slots.Icon;
  if (!theme || !Custom) return <>{children}</>;
  return (
    <SlotBoundary fallback={children} themeId={theme.id} where="the slot Icon">
      <Custom name={name} />
    </SlotBoundary>
  );
}
