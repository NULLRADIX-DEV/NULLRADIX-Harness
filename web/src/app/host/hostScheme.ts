import type { ColorSchemeState } from '@nullradix/theme-sdk';
import { useMemo, useSyncExternalStore } from 'react';
import { useTheme } from '../../shared/ui/ThemeProvider';

function subscribe(listener: () => void): () => void {
  const observer = new MutationObserver(listener);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['data-theme', 'data-theme-mode'],
  });
  return () => {
    observer.disconnect();
  };
}

// Scheme and mode of the page in one string, so one observer serves both.
function snapshot(): string {
  const { theme, themeMode } = document.documentElement.dataset;
  return `${theme === 'light' ? 'light' : 'dark'} ${themeMode ?? ''}`;
}

// Light or dark as the page shows it, also for the system setting and safe mode.
export function useHostColorScheme(): ColorSchemeState {
  const { mode } = useTheme();
  const shown = useSyncExternalStore(subscribe, snapshot);
  return useMemo(() => {
    const [scheme, shownMode] = shown.split(' ');
    return {
      scheme: scheme === 'light' ? 'light' : 'dark',
      themeId: mode,
      mode: shownMode === '' || shownMode === undefined ? mode : shownMode,
    };
  }, [shown, mode]);
}
