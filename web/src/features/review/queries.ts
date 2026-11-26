import { queryOptions } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { ticketKeys } from '../tickets/queries';

export type Review = components['schemas']['ReviewDto'];
export type ReviewFile = components['schemas']['ReviewFileDto'];
export type ReviewPatch = components['schemas']['ReviewPatchDto'];
export type ReviewFileContent = components['schemas']['ReviewFileContentDto'];
export type TestRun = components['schemas']['TestRunDto'];

// Below the ticket keys, so every change of a ticket reloads its review as well.
export const reviewKeys = {
  all: [...ticketKeys.all, 'review'] as const,
  of: (ticketKey: string) => [...reviewKeys.all, ticketKey] as const,
  patch: (ticketKey: string, path: string) => [...reviewKeys.of(ticketKey), 'patch', path] as const,
  file: (ticketKey: string, path: string) => [...reviewKeys.of(ticketKey), 'file', path] as const,
};

// A review is opened on the worktree, and its tests come from a run.
export const reviewLive: ChangeRegistry = {
  worktree: [reviewKeys.all],
  run: [reviewKeys.all],
};

export function reviewQuery(ticketKey: string) {
  return queryOptions({
    queryKey: reviewKeys.of(ticketKey),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/tickets/{ticketKey}/review', {
          params: { path: { ticketKey } },
        }),
      ),
  });
}

export function patchQuery(ticketKey: string, path: string) {
  return queryOptions({
    queryKey: reviewKeys.patch(ticketKey, path),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/tickets/{ticketKey}/review/diff', {
          params: { path: { ticketKey }, query: { path } },
        }),
      ),
  });
}

// The route has a wildcard path, which the client does not fill in, so the path is made here.
type FileRoute = '/api/v1/tickets/{ticketKey}/review/files/{*path}';

export function fileQuery(ticketKey: string, path: string) {
  const segments = path.split('/').map(encodeURIComponent).join('/');
  const url = `/api/v1/tickets/${encodeURIComponent(ticketKey)}/review/files/${segments}`;
  return queryOptions({
    queryKey: reviewKeys.file(ticketKey, path),
    queryFn: async () =>
      unwrap(
        await api.GET(url as FileRoute, {
          params: { path: { ticketKey, path } },
        }),
      ),
  });
}
