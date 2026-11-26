import { queryOptions, useMutation } from '@tanstack/react-query';
import { api, type components } from '../api/client';
import { unwrap, unwrapEmpty } from '../api/errors';
import type { ChangeRegistry } from '../live/registry';
import { useInvalidate } from './useInvalidate';
import { themeKeys } from './themes';

export type CatalogSource = components['schemas']['CatalogSourceDto'];
export type CatalogSourceStatus = components['schemas']['CatalogSourceStatusDto'];
export type CatalogTheme = components['schemas']['CatalogThemeDto'];

export const catalogKeys = {
  sources: ['theme-catalog', 'sources'] as const,
  themes: ['theme-catalog', 'themes'] as const,
};

// Catalog themes come from the network and refetch only on an action, never on every change.
export const catalogLive: ChangeRegistry = {
  setting: [catalogKeys.sources],
};

export function catalogSourcesQuery() {
  return queryOptions({
    queryKey: catalogKeys.sources,
    queryFn: async () => unwrap(await api.GET('/api/v1/theme-catalog/sources')),
  });
}

export function catalogThemesQuery(enabled: boolean) {
  return queryOptions({
    queryKey: catalogKeys.themes,
    queryFn: async () => unwrap(await api.GET('/api/v1/theme-catalog/themes')),
    enabled,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });
}

export function useAddCatalogSource() {
  const invalidateSources = useInvalidate(catalogKeys.sources);
  const invalidateThemes = useInvalidate(catalogKeys.themes);
  return useMutation({
    mutationFn: async (url: string) =>
      unwrap(await api.POST('/api/v1/theme-catalog/sources', { body: { url } })),
    onSettled: async () => {
      await Promise.all([invalidateSources(), invalidateThemes()]);
    },
  });
}

export function useRemoveCatalogSource() {
  const invalidateSources = useInvalidate(catalogKeys.sources);
  const invalidateThemes = useInvalidate(catalogKeys.themes);
  return useMutation({
    mutationFn: async ({ id, version }: { id: string; version: number }) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/theme-catalog/sources/{id}', {
          params: { path: { id }, query: { version } },
        }),
      );
    },
    onSettled: async () => {
      await Promise.all([invalidateSources(), invalidateThemes()]);
    },
  });
}

export function useInstallFromCatalog() {
  const invalidateThemes = useInvalidate(themeKeys.all);
  const invalidateCatalog = useInvalidate(catalogKeys.themes);
  return useMutation({
    mutationFn: async ({
      sourceId,
      id,
      replace,
    }: {
      sourceId: string;
      id: string;
      replace?: boolean;
    }) =>
      unwrap(
        await api.POST('/api/v1/theme-catalog/sources/{id}/themes/{theme}/install', {
          params: {
            path: { id: sourceId, theme: id },
            query: replace === true ? { replace: true } : {},
          },
        }),
      ),
    onSettled: async () => {
      await Promise.all([invalidateThemes(), invalidateCatalog()]);
    },
  });
}
