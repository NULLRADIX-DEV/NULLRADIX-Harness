import { queryOptions, useMutation } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';

export type Run = components['schemas']['RunDto'];
export type RunStatus = components['schemas']['RunStatusDto'];
export type OutputLine = components['schemas']['OutputLineDto'];

export const RUN_LIST_LIMIT = 50;
// The page keeps this many lines of output; older ones scroll out.
export const OUTPUT_LINE_LIMIT = 5000;
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
  failing: boolean;
}

// Follows the output of one run until it ends; mount it per run so state starts empty.
export function useRunOutput(runId: string): RunOutputState {
  const [state, setState] = useState<RunOutputState>({
    lines: [],
    truncated: false,
    finished: false,
    failing: false,
  });

  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let after = 0;

    async function poll() {
      try {
        const page = unwrap(
          await api.GET('/api/v1/runs/{id}/output', {
            params: { path: { id: runId }, query: { after } },
          }),
        );
        if (stopped) return;
        const last = page.lines.at(-1);
        if (last) after = last.seq;
        setState((previous) => {
          const lines = previous.lines.concat(page.lines);
          const truncated = previous.truncated || lines.length > OUTPUT_LINE_LIMIT;
          return {
            lines: lines.length > OUTPUT_LINE_LIMIT ? lines.slice(-OUTPUT_LINE_LIMIT) : lines,
            truncated,
            finished: page.finished,
            failing: false,
          };
        });
        if (page.finished) return;
        // a full page means more is waiting right away
        timer = setTimeout(() => void poll(), last ? 0 : OUTPUT_POLL_MS);
      } catch {
        if (stopped) return;
        setState((previous) => ({ ...previous, failing: true }));
        timer = setTimeout(() => void poll(), OUTPUT_RETRY_MS);
      }
    }

    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [runId]);

  return state;
}
