import { queryOptions } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';

export type MergeJob = components['schemas']['MergeJobDto'];
export type MergeRetry = components['schemas']['MergeRetryDto'];

export const mergeKeys = {
  all: ['merges'] as const,
  of: (ticketKey: string) => [...mergeKeys.all, ticketKey] as const,
};

// A job is announced as a change of its worktree; the ticket carries the block it ends in.
export const mergeLive: ChangeRegistry = {
  worktree: [mergeKeys.all],
  ticket: [mergeKeys.all],
};

export function mergesQuery(ticketKey: string) {
  return queryOptions({
    queryKey: mergeKeys.of(ticketKey),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/tickets/{ticketKey}/merges', {
          params: { path: { ticketKey } },
        }),
      ).items,
  });
}
