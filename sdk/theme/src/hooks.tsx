import { createContext, useContext, type ReactNode } from 'react';
import type { ThemeHost } from './host.js';
import type { ThemeSettingsSchema, ThemeSettingsState, ThemeSettingValues } from './settings.js';

// Context with the host implementation; null outside the provider
const ThemeHostContext = createContext<ThemeHost | null>(null);

/** Gives themes the host implementation (only the NULLRADIX host renders it) */
export function ThemeHostProvider({ value, children }: { value: ThemeHost; children?: ReactNode }) {
  return <ThemeHostContext.Provider value={value}>{children}</ThemeHostContext.Provider>;
}

// Host from the context; outside the provider an error naming hook and provider (internal)
export function useHost(name: string): ThemeHost {
  const host = useContext(ThemeHostContext);
  if (!host) {
    throw new Error(
      `${name} needs the ThemeHostProvider of the NULLRADIX host - the component runs outside the NULLRADIX interface.`,
    );
  }
  return host;
}

// Host from the context, or null outside the provider (internal)
export function useOptionalHost(): ThemeHost | null {
  return useContext(ThemeHostContext);
}

/** Entries of the main navigation */
export const useNav = () => useHost('useNav').nav;
/** Projects, boards and the board choice */
export const useBoards = () => useHost('useBoards').boards;
/** Claude usage */
export const useUsage = () => useHost('useUsage').usage;
/** Read and change the user settings */
export const useSettings = () => useHost('useSettings').settings;
/** Read and change the theme settings; `S` is the `settingsSchema` of the theme */
export function useThemeSettings<
  S extends ThemeSettingsSchema = ThemeSettingsSchema,
>(): ThemeSettingsState & { values: ThemeSettingValues<S> } {
  return useHost('useThemeSettings').themeSettings as ThemeSettingsState & {
    values: ThemeSettingValues<S>;
  };
}
/** The shown color scheme */
export const useColorScheme = () => useHost('useColorScheme').colorScheme;
/** Change the route */
export const useNavigate = () => useHost('useNavigate').navigate;
/** Open the global search of the host */
export const useOpenSearch = () => useHost('useOpenSearch').openSearch;
