import { queryOptions } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';

export type Board = components['schemas']['BoardDto'];

export const boardKeys = {
  all: ['boards'] as const,
  list: () => [...boardKeys.all, 'list'] as const,
};

export const boardsLive: ChangeRegistry = {
  board: [boardKeys.all],
};

export function boardsQuery() {
  return queryOptions({
    queryKey: boardKeys.list(),
    queryFn: async () => unwrap(await api.GET('/api/v1/boards')).items,
  });
}
