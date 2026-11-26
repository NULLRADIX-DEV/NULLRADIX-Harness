import { queryOptions, useMutation, useQueryClient } from '@tanstack/react-query';
import { api, type components } from '../api/client';
import { unwrap, unwrapEmpty } from '../api/errors';
import type { ChangeRegistry } from '../live/registry';
import { useInvalidate } from './useInvalidate';

export type Theme = components['schemas']['ThemeDto'];
export type ThemeSettingSpec = components['schemas']['ThemeSettingDto'];
export type ThemeSettings = components['schemas']['ThemeSettingsDto'];
export type ClaudeTheme = components['schemas']['ClaudeThemeDto'];

export const themeKeys = {
  all: ['themes'] as const,
  list: () => [...themeKeys.all, 'list'] as const,
  settings: (id: string) => [...themeKeys.all, 'settings', id] as const,
  claude: () => [...themeKeys.all, 'claude'] as const,
};

// The chosen theme is a setting of the daemon, so its changes arrive as setting changes.
export const themesLive: ChangeRegistry = {
  theme: [themeKeys.all],
  setting: [themeKeys.all],
};

export function themesQuery() {
  return queryOptions({
    queryKey: themeKeys.list(),
    queryFn: async () => unwrap(await api.GET('/api/v1/themes')),
  });
}

export function useChooseTheme() {
  const invalidate = useInvalidate(themeKeys.all);
  return useMutation({
    mutationFn: async (theme: string) =>
      unwrap(await api.PUT('/api/v1/settings/theme', { body: { theme } })),
    onSettled: invalidate,
  });
}

export function useInstallTheme() {
  const invalidate = useInvalidate(themeKeys.all);
  return useMutation({
    mutationFn: async (file: Blob) =>
      unwrap(
        await api.POST('/api/v1/themes', {
          // the schema types a byte list, the body is the file itself
          body: [],
          bodySerializer: () => file,
          headers: { 'content-type': 'application/octet-stream' },
        }),
      ),
    onSettled: invalidate,
  });
}

export function useRemoveTheme() {
  const invalidate = useInvalidate(themeKeys.all);
  return useMutation({
    mutationFn: async ({ id, version }: { id: string; version: number }) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/themes/{id}', {
          params: { path: { id }, query: { version } },
        }),
      );
    },
    onSettled: invalidate,
  });
}

export function themeSettingsQuery(id: string) {
  return queryOptions({
    queryKey: themeKeys.settings(id),
    queryFn: async () =>
      unwrap(await api.GET('/api/v1/themes/{id}/settings', { params: { path: { id } } })),
  });
}

export function useSaveThemeSettings(id: string) {
  const invalidate = useInvalidate(themeKeys.all);
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (input: ThemeSettings) =>
      unwrap(
        await api.PUT('/api/v1/themes/{id}/settings', {
          params: { path: { id } },
          body: input,
        }),
      ),
    onSuccess: (saved) => {
      queryClient.setQueryData(themeKeys.settings(id), saved);
    },
    onSettled: invalidate,
  });
}

export function useResetThemeSettings(id: string) {
  const invalidate = useInvalidate(themeKeys.all);
  return useMutation({
    mutationFn: async (version: number) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/themes/{id}/settings', {
          params: { path: { id }, query: { version } },
        }),
      );
    },
    onSettled: invalidate,
  });
}

export function claudeThemeQuery() {
  return queryOptions({
    queryKey: themeKeys.claude(),
    queryFn: async () => unwrap(await api.GET('/api/v1/settings/claude-theme')),
  });
}

export function useSetClaudeTheme() {
  const invalidate = useInvalidate(themeKeys.all);
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (enabled: boolean) =>
      unwrap(await api.PUT('/api/v1/settings/claude-theme', { body: { enabled } })),
    onSuccess: (status) => {
      queryClient.setQueryData(themeKeys.claude(), status);
    },
    onSettled: invalidate,
  });
}
