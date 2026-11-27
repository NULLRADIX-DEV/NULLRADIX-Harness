import type { NavItem } from '@nullradix/theme-sdk';
import { useMemo } from 'react';
import { useLocation } from 'react-router';
import { NAV_ITEMS } from '../layout/navigation';

// The main navigation as themes see it; the entry of the open page is active.
export function useHostNav(): NavItem[] {
  const { pathname } = useLocation();
  return useMemo(
    () =>
      NAV_ITEMS.map(({ id, label, path }) => ({
        id,
        label,
        href: path,
        active: pathname === path || pathname.startsWith(`${path}/`),
      })),
    [pathname],
  );
}
