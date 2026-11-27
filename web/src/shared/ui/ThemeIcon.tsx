import type { IconName, NavIconName } from '@nullradix/theme-sdk';
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

// An icon of the main navigation that the slot NavIcon may replace.
export function NavIcon({ name, children = null }: { name: NavIconName; children?: ReactNode }) {
  const theme = useSlotTheme();
  const Custom = theme?.slots.NavIcon;
  if (!theme || !Custom) return <>{children}</>;
  return (
    <SlotBoundary fallback={children} themeId={theme.id} where="the slot NavIcon">
      <Custom name={name} />
    </SlotBoundary>
  );
}
