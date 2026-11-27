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

// Light or dark as the page shows it, also for the system setting and safe mode.
export function useHostColorScheme(): ColorSchemeState {
  const { mode } = useTheme();
  const scheme = useSyncExternalStore(subscribe, () =>
    document.documentElement.dataset['theme'] === 'light' ? 'light' : 'dark',
  );
  const shown = useSyncExternalStore(
    subscribe,
    () => document.documentElement.dataset['themeMode'],
  );
  return useMemo(() => ({ scheme, themeId: mode, mode: shown ?? mode }), [scheme, mode, shown]);
}
