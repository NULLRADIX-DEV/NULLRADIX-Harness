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
  // only paths of the app: a theme does not send the person to another site
  const navigate = useCallback(
    (to: string) => {
      if (typeof to === 'string' && to.startsWith('/') && !to.startsWith('//')) {
        void routerNavigate(to);
      }
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
