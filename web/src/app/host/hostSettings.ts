import type { SettingsState, ThemeSettings, ThemeSettingsState } from '@nullradix/theme-sdk';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useMemo, useRef } from 'react';
import { isThemeMode } from '../../shared/lib/theme';
import { resolveThemeSettings, type ThemeSettingValue } from '../../shared/lib/themeSettings';
import {
  themeSettingsQuery,
  themesQuery,
  useChooseTheme,
  useSaveThemeSettings,
} from '../../shared/query/themes';
import { useTheme } from '../../shared/ui/ThemeProvider';

// The user settings; a theme may switch only to a built-in theme, which has no code or Claude part.
export function useHostSettings(): SettingsState {
  const themes = useQuery(themesQuery());
  const { mutateAsync: choose } = useChooseTheme();
  const active = themes.data?.active;
  const update = useCallback(
    async (patch: Partial<ThemeSettings>) => {
      const theme: unknown = patch.theme;
      if (theme === undefined) return;
      if (!isThemeMode(theme)) {
        throw new Error('A theme can switch only to nullradix, light, dark or system.');
      }
      await choose(theme);
    },
    [choose],
  );
  return useMemo(
    () => ({
      values: active === undefined ? null : { theme: active },
      loading: active === undefined,
      update,
    }),
    [active, update],
  );
}

// The values of the settings the manifest of the shown theme declares; saves go one after another.
export function useHostThemeSettings(active: boolean): ThemeSettingsState {
  const { mode, installed } = useTheme();
  const queryClient = useQueryClient();
  const specs = useMemo(
    () => installed.find((theme) => theme.id === mode)?.settings ?? [],
    [installed, mode],
  );
  const stored = useQuery({
    ...themeSettingsQuery(mode),
    enabled: active && specs.length > 0,
  });
  const { mutateAsync: save } = useSaveThemeSettings(mode);
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  const values = useMemo(
    () => resolveThemeSettings(specs, stored.data?.values ?? {}),
    [specs, stored.data],
  );
  const set = useCallback(
    (key: string, value: boolean | string) => {
      const run = async () => {
        if (!specs.some((spec) => spec.key === key)) {
          throw new Error(`The theme declares no setting "${key}".`);
        }
        const current = await queryClient.query(themeSettingsQuery(mode));
        const next: Record<string, ThemeSettingValue> = { ...current.values, [key]: value };
        await save({ values: next, version: current.version });
      };
      const done = queue.current.then(run, run);
      queue.current = done.catch(() => undefined);
      return done;
    },
    [mode, queryClient, save, specs],
  );
  return useMemo(
    () => ({ values, loading: specs.length > 0 && stored.data === undefined, set }),
    [values, specs.length, stored.data, set],
  );
}
