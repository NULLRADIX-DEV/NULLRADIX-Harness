import { queryOptions } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import { dispatcherKeys } from '../dispatcher/queries';
import type { StatsParams } from './range';

export type DispatchStats = components['schemas']['DispatchStatsDto'];
export type ProfileRow = components['schemas']['ProfileRowDto'];

export const statsKeys = {
  range: (params: StatsParams) => [...dispatcherKeys.stats(), params] as const,
};

export function statsQuery(params: StatsParams) {
  return queryOptions({
    queryKey: statsKeys.range(params),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/stats/dispatch', {
          params: {
            query: {
              from: params.from,
              to: params.to,
              kind: params.kind,
              ...(params.boardId === null ? {} : { boardId: params.boardId }),
            },
          },
        }),
      ),
  });
}

// Readiness of the tickets, for one board or all; under the status key so its live updates apply.
export function boardStatusQuery(boardId: string | null) {
  return queryOptions({
    queryKey: [...dispatcherKeys.status(), boardId ?? 'all'] as const,
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/dispatcher', {
          params: { query: boardId === null ? {} : { boardId } },
        }),
      ),
  });
}
