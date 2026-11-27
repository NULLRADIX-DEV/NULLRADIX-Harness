import { useQuery } from '@tanstack/react-query';
import {
  createContext,
  use,
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import {
  DARK_SCHEME_QUERY,
  DEFAULT_THEME_MODE,
  applyInstalledTheme,
  applyTheme,
  isThemeMode,
  readInstalledLook,
  readThemeChoice,
  removeThemeStylesheet,
  saveInstalledLook,
  saveThemeMode,
  systemPrefersDark,
  type InstalledLook,
} from '../lib/theme';
import { isSafeTheme } from '../lib/safeTheme';
import { themesQuery, useChooseTheme, type Theme } from '../query/themes';

interface ThemeState {
  // A built-in theme or the id of an installed one.
  mode: string;
  setMode: (mode: string) => void;
  installed: readonly Theme[];
}

const ThemeContext = createContext<ThemeState | null>(null);

function lookOf(theme: Theme): InstalledLook {
  return {
    id: theme.id,
    scheme: theme.colorScheme,
    styleUrl: theme.styleUrl,
    startColor: theme.startColor ?? null,
  };
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<string>(() => readThemeChoice(window.localStorage));
  const themes = useQuery(themesQuery());
  const { mutate: chooseTheme } = useChooseTheme();
  const installed = useMemo(() => themes.data?.items ?? [], [themes.data]);
  const serverActive = themes.data?.active;

  // the daemon holds the choice; a change made elsewhere shows here too
  const [seenActive, setSeenActive] = useState<string | undefined>(undefined);
  if (serverActive !== undefined && serverActive !== seenActive) {
    setSeenActive(serverActive);
    setModeState(serverActive);
  }

  const theme = installed.find((candidate) => candidate.id === mode);

  useEffect(() => {
    const root = document.documentElement;
    saveThemeMode(window.localStorage, mode);
    if (isThemeMode(mode)) {
      removeThemeStylesheet();
      saveInstalledLook(window.localStorage, null);
      applyTheme(root, mode, systemPrefersDark());
      if (mode !== 'system') return;
      const media = window.matchMedia(DARK_SCHEME_QUERY);
      const follow = (event: MediaQueryListEvent) => {
        applyTheme(root, mode, event.matches);
      };
      media.addEventListener('change', follow);
      return () => {
        media.removeEventListener('change', follow);
      };
    }
    if (isSafeTheme()) {
      // safe mode: an installed theme shows light or dark by the system, without its CSS
      removeThemeStylesheet();
      applyTheme(root, 'system', systemPrefersDark());
      root.dataset['themeMode'] = mode;
      return undefined;
    }
    // an installed theme shows from its stored look until the list arrives
    const look = theme ? lookOf(theme) : readInstalledLook(window.localStorage, mode);
    if (look) {
      applyInstalledTheme(root, look);
      saveInstalledLook(window.localStorage, look);
    } else if (themes.data) {
      // the theme is gone, the daemon already answers with the default
      removeThemeStylesheet();
      applyTheme(root, DEFAULT_THEME_MODE, systemPrefersDark());
    }
    return undefined;
  }, [mode, theme, themes.data]);

  const setMode = useCallback(
    (next: string) => {
      saveThemeMode(window.localStorage, next);
      setModeState(next);
      chooseTheme(next, {
        onError: () => {
          if (serverActive !== undefined) {
            saveThemeMode(window.localStorage, serverActive);
            setModeState(serverActive);
          }
        },
      });
    },
    [chooseTheme, serverActive],
  );

  const value = useMemo(() => ({ mode, setMode, installed }), [mode, setMode, installed]);
  return <ThemeContext value={value}>{children}</ThemeContext>;
}

export function useTheme(): ThemeState {
  const state = use(ThemeContext);
  if (!state) throw new Error('useTheme must be used inside a ThemeProvider');
  return state;
}
