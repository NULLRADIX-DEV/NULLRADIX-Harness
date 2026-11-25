import { queryOptions } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';

export type Session = components['schemas']['SessionDto'];
export type DisplayState = components['schemas']['DisplayStateDto'];
export type Subagent = components['schemas']['SubagentDto'];
export type SessionClient = components['schemas']['SessionClientDto'];
export type SessionDetail = components['schemas']['SessionDetailDto'];
export type TranscriptEntry = components['schemas']['TranscriptEntryDto'];
export type TranscriptPage = components['schemas']['TranscriptPageDto'];
export type SessionChanges = components['schemas']['SessionChangesDto'];
export type ChangedFile = components['schemas']['ChangedFileDto'];
export type FileDiff = components['schemas']['FileDiffDto'];
export type DiffHunk = components['schemas']['DiffHunkDto'];

export const sessionKeys = {
  all: ['sessions'] as const,
  list: () => [...sessionKeys.all, 'list'] as const,
  ticket: (key: string) => [...sessionKeys.all, 'ticket', key] as const,
  tickets: () => [...sessionKeys.all, 'ticket'] as const,
  detail: (id: string) => [...sessionKeys.all, 'detail', id] as const,
  details: () => [...sessionKeys.all, 'detail'] as const,
  changes: (id: string) => [...sessionKeys.all, 'changes', id] as const,
  diffs: (id: string) => [...sessionKeys.all, 'diff', id] as const,
  diff: (id: string, path: string) => [...sessionKeys.diffs(id), path] as const,
};

// No session id in changes: lists reload, an open detail polls, its git changes follow growth.
export const sessionsLive: ChangeRegistry = {
  session: [sessionKeys.list(), sessionKeys.tickets()],
  ticket: [sessionKeys.list(), sessionKeys.tickets(), sessionKeys.details()],
};

export function sessionsQuery() {
  return queryOptions({
    queryKey: sessionKeys.list(),
    queryFn: async () => unwrap(await api.GET('/api/v1/sessions')).items,
  });
}

export function ticketSessionsQuery(ticketKey: string) {
  return queryOptions({
    queryKey: sessionKeys.ticket(ticketKey),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/tickets/{ticketKey}/sessions', {
          params: { path: { ticketKey } },
        }),
      ).items,
  });
}

// The feed does not name sessions, so a detail is fetched again this often until it ended.
export const LIVE_REFRESH_MS = 3000;

export function sessionDetailQuery(sessionId: string) {
  return queryOptions({
    queryKey: sessionKeys.detail(sessionId),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/sessions/{sessionId}/detail', {
          params: { path: { sessionId } },
        }),
      ),
    refetchInterval: (query) =>
      query.state.data?.session.displayState === 'ended' ? false : LIVE_REFRESH_MS,
  });
}

export function sessionChangesQuery(sessionId: string) {
  return queryOptions({
    queryKey: sessionKeys.changes(sessionId),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/sessions/{sessionId}/changes', {
          params: { path: { sessionId } },
        }),
      ),
  });
}

export function fileDiffQuery(sessionId: string, path: string) {
  return queryOptions({
    queryKey: sessionKeys.diff(sessionId, path),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/sessions/{sessionId}/changes/file', {
          params: { path: { sessionId }, query: { path } },
        }),
      ),
  });
}

export async function fetchTranscript(
  sessionId: string,
  after: number,
  limit: number,
): Promise<TranscriptPage> {
  return unwrap(
    await api.GET('/api/v1/sessions/{sessionId}/transcript', {
      params: { path: { sessionId }, query: { after, limit } },
    }),
  );
}
