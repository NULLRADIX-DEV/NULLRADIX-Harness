import { queryOptions, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { api, type components } from '../../shared/api/client';
import { ApiProblemError, isUnauthorized, unwrap } from '../../shared/api/errors';
import { recheckSession } from '../session/queries';
import { runKeys, type Run } from './queries';

export type RunDetail = components['schemas']['RunDetailDto'];
export type TranscriptLine = components['schemas']['TranscriptLineDto'];
export type LineKind = components['schemas']['LineKindDto'];
export type Screenshot = components['schemas']['ScreenshotDto'];

export const TRANSCRIPT_PAGE = 1000;
export const TRANSCRIPT_POLL_MS = 1000;
export const TRANSCRIPT_RETRY_MS = 3000;

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function isUuid(text: string | null | undefined): text is string {
  return text !== null && text !== undefined && UUID.test(text);
}

export function screenshotUrl(runId: string, screenshotId: string): string {
  return `/api/v1/runs/${runId}/screenshots/${screenshotId}`;
}

export function ticketRunsQuery(ticketKey: string) {
  return queryOptions({
    queryKey: [...runKeys.all, 'ticket', ticketKey] as const,
    queryFn: async () =>
      unwrap(await api.GET('/api/v1/runs', { params: { query: { ticketKey } } })).items,
  });
}

// The run that started a Claude Code session; null when none is recorded.
export function sessionRunQuery(sessionId: string, runName: string | null | undefined) {
  return queryOptions({
    queryKey: [...runKeys.all, 'session', sessionId, runName ?? ''] as const,
    enabled: isUuid(runName),
    queryFn: async (): Promise<Run | null> => {
      if (!isUuid(runName)) return null;
      const list = unwrap(
        await api.GET('/api/v1/runs', { params: { query: { sessionId, runId: runName } } }),
      );
      return list.items[0] ?? null;
    },
  });
}

export function runDetailQuery(runId: string) {
  return queryOptions({
    queryKey: [...runKeys.all, 'detail', runId] as const,
    queryFn: async () =>
      unwrap(await api.GET('/api/v1/runs/{id}', { params: { path: { id: runId } } })),
  });
}

export function useRunDetail(runId: string) {
  return useQuery(runDetailQuery(runId));
}

export interface RunTranscriptState {
  lines: TranscriptLine[];
  finished: boolean;
  retrying: boolean;
  refused: ApiProblemError | null;
}

// Reads the transcript of one run page by page; an active run is polled until it ends.
export function useRunTranscript(runId: string, active: boolean): RunTranscriptState {
  const client = useQueryClient();
  const activeRef = useRef(active);
  useEffect(() => {
    activeRef.current = active;
  }, [active]);
  const [state, setState] = useState<RunTranscriptState>({
    lines: [],
    finished: false,
    retrying: false,
    refused: null,
  });

  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let after = 0;

    async function poll() {
      try {
        const page = unwrap(
          await api.GET('/api/v1/runs/{id}/transcript', {
            params: { path: { id: runId }, query: { after, limit: TRANSCRIPT_PAGE } },
          }),
        );
        if (stopped) return;
        after = page.lastSeq;
        const full = page.lines.length >= TRANSCRIPT_PAGE;
        // a run that ended before this page was read has nothing more to wait for
        const done = page.finished || (!activeRef.current && !full);
        setState((previous) => ({
          lines: previous.lines.concat(page.lines),
          finished: done,
          retrying: false,
          refused: null,
        }));
        if (done) {
          if (activeRef.current) void client.invalidateQueries({ queryKey: runKeys.all });
          return;
        }
        timer = setTimeout(() => void poll(), full ? 0 : TRANSCRIPT_POLL_MS);
      } catch (error) {
        if (stopped) return;
        if (error instanceof ApiProblemError && error.status < 500) {
          if (isUnauthorized(error)) void recheckSession(client);
          setState((previous) => ({ ...previous, retrying: false, refused: error }));
          return;
        }
        setState((previous) => ({ ...previous, retrying: true }));
        timer = setTimeout(() => void poll(), TRANSCRIPT_RETRY_MS);
      }
    }

    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [client, runId]);

  return state;
}
