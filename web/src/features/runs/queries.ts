import { queryOptions, useMutation, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { api, type components } from '../../shared/api/client';
import { ApiProblemError, isUnauthorized, unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';
import { recheckSession } from '../session/queries';

export type Run = components['schemas']['RunDto'];
export type RunStatus = components['schemas']['RunStatusDto'];
export type OutputLine = components['schemas']['OutputLineDto'];

export const RUN_LIST_LIMIT = 50;
// The page keeps this many lines of output; older ones scroll out.
export const OUTPUT_LINE_LIMIT = 5000;
export const OUTPUT_PAGE = 500;
export const OUTPUT_POLL_MS = 1000;
export const OUTPUT_RETRY_MS = 3000;

export const runKeys = {
  all: ['runs'] as const,
  list: () => [...runKeys.all, 'list'] as const,
};

export const runsLive: ChangeRegistry = {
  run: [runKeys.all],
};

export function isActive(status: RunStatus): boolean {
  return status === 'starting' || status === 'running';
}

export function runsQuery() {
  return queryOptions({
    queryKey: runKeys.list(),
    queryFn: async () =>
      unwrap(await api.GET('/api/v1/runs', { params: { query: { limit: RUN_LIST_LIMIT } } })).items,
  });
}

export function useCancelRun() {
  const invalidate = useInvalidate(runKeys.all);
  return useMutation({
    mutationFn: async (id: string) =>
      unwrap(await api.POST('/api/v1/runs/{id}/cancel', { params: { path: { id } } })),
    onSettled: invalidate,
  });
}

export interface RunOutputState {
  lines: OutputLine[];
  // Earlier lines were dropped to stay within OUTPUT_LINE_LIMIT.
  truncated: boolean;
  finished: boolean;
  // The daemon cannot be reached right now; polling goes on.
  retrying: boolean;
  // The daemon refused the output; polling has stopped.
  refused: ApiProblemError | null;
}

// Follows the output of one run until it ends; mount it per run so state starts empty.
export function useRunOutput(runId: string): RunOutputState {
  const client = useQueryClient();
  const [state, setState] = useState<RunOutputState>({
    lines: [],
    truncated: false,
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
          await api.GET('/api/v1/runs/{id}/output', {
            params: { path: { id: runId }, query: { after, limit: OUTPUT_PAGE } },
          }),
        );
        if (stopped) return;
        const last = page.lines.at(-1);
        if (last) after = last.seq;
        setState((previous) => {
          const lines = previous.lines.concat(page.lines);
          return {
            lines: lines.length > OUTPUT_LINE_LIMIT ? lines.slice(-OUTPUT_LINE_LIMIT) : lines,
            truncated: previous.truncated || lines.length > OUTPUT_LINE_LIMIT,
            finished: page.finished,
            retrying: false,
            refused: null,
          };
        });
        if (page.finished) {
          // the list may not have heard of the end yet
          void client.invalidateQueries({ queryKey: runKeys.all });
          return;
        }
        // a full page means more is waiting right away
        const full = page.lines.length >= OUTPUT_PAGE;
        timer = setTimeout(() => void poll(), full ? 0 : OUTPUT_POLL_MS);
      } catch (error) {
        if (stopped) return;
        if (error instanceof ApiProblemError && error.status < 500) {
          if (isUnauthorized(error)) void recheckSession(client);
          setState((previous) => ({ ...previous, retrying: false, refused: error }));
          return;
        }
        setState((previous) => ({ ...previous, retrying: true }));
        timer = setTimeout(() => void poll(), OUTPUT_RETRY_MS);
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
