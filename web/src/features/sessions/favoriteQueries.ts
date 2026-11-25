import { queryOptions, useMutation } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap, unwrapEmpty } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';

export type Favorite = components['schemas']['FavoriteDto'];

export const favoriteKeys = {
  all: ['favorites'] as const,
  list: () => [...favoriteKeys.all, 'list'] as const,
};

// Favorites show the live state of their sessions, so session changes reload them too.
export const favoritesLive: ChangeRegistry = {
  favorite: [favoriteKeys.all],
  session: [favoriteKeys.all],
  ticket: [favoriteKeys.all],
};

export function favoritesQuery() {
  return queryOptions({
    queryKey: favoriteKeys.list(),
    queryFn: async () => unwrap(await api.GET('/api/v1/favorites')).items,
  });
}

export interface MarkVars {
  sessionId: string;
  // Replaces the note; left out keeps it.
  note?: string;
}

export function useMarkFavorite() {
  const changed = useInvalidate(favoriteKeys.all);
  return useMutation({
    mutationFn: async ({ sessionId, note }: MarkVars) =>
      unwrap(
        await api.PUT('/api/v1/sessions/{sessionId}/favorite', {
          params: { path: { sessionId } },
          body: note === undefined ? {} : { note },
        }),
      ),
    onSettled: changed,
  });
}

export function useUnmarkFavorite() {
  const changed = useInvalidate(favoriteKeys.all);
  return useMutation({
    mutationFn: async (sessionId: string) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/sessions/{sessionId}/favorite', {
          params: { path: { sessionId } },
        }),
      );
    },
    onSettled: changed,
  });
}
