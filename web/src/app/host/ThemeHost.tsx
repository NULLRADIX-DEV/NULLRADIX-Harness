import { ThemeHostProvider, type ShellParts, type ThemeHost } from '@nullradix/theme-sdk';
import { useCallback, useMemo, type ReactNode } from 'react';
import { useNavigate } from 'react-router';
import { useSlotTheme } from '../../shared/ui/useSlot';
import { useHostBoards } from './hostBoards';
import { useHostNav } from './hostNav';
import { useHostColorScheme } from './hostScheme';
import { useHostSettings, useHostThemeSettings } from './hostSettings';
import { useHostUsage } from './hostUsage';

const openNothing = () => undefined;

// What theme code may use of the app; values read the query cache, so live changes show.
export function NullradixThemeHost({
  parts,
  children,
}: {
  parts: ShellParts;
  children: ReactNode;
}) {
  // nothing is fetched for the host while no theme code is shown
  const active = useSlotTheme() !== undefined;
  const routerNavigate = useNavigate();
  const nav = useHostNav();
  const boards = useHostBoards(active);
  const usage = useHostUsage(active);
  const settings = useHostSettings();
  const themeSettings = useHostThemeSettings(active);
  const colorScheme = useHostColorScheme();
  // only places of the app: the address is resolved like the browser does, backslashes included
  const navigate = useCallback(
    (to: string) => {
      if (typeof to !== 'string' || !to.startsWith('/')) return;
      const url = new URL(to, window.location.origin);
      if (url.origin !== window.location.origin || url.pathname.startsWith('//')) return;
      void routerNavigate(`${url.pathname}${url.search}${url.hash}`);
    },
    [routerNavigate],
  );
  const host = useMemo<ThemeHost>(
    () => ({
      nav,
      boards,
      usage,
      settings,
      themeSettings,
      colorScheme,
      navigate,
      // the app has no global search yet
      openSearch: openNothing,
      parts,
    }),
    [nav, boards, usage, settings, themeSettings, colorScheme, navigate, parts],
  );
  return <ThemeHostProvider value={host}>{children}</ThemeHostProvider>;
}
