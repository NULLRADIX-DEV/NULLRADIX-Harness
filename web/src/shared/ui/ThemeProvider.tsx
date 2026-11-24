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
  applyTheme,
  readThemeMode,
  saveThemeMode,
  systemPrefersDark,
  type ThemeMode,
} from '../lib/theme';

interface ThemeState {
  mode: ThemeMode;
  setMode: (mode: ThemeMode) => void;
}

const ThemeContext = createContext<ThemeState | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<ThemeMode>(() => readThemeMode(window.localStorage));

  useEffect(() => {
    const root = document.documentElement;
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
  }, [mode]);

  const setMode = useCallback((next: ThemeMode) => {
    saveThemeMode(window.localStorage, next);
    setModeState(next);
  }, []);

  const value = useMemo(() => ({ mode, setMode }), [mode, setMode]);
  return <ThemeContext value={value}>{children}</ThemeContext>;
}

export function useTheme(): ThemeState {
  const state = use(ThemeContext);
  if (!state) throw new Error('useTheme must be used inside a ThemeProvider');
  return state;
}
