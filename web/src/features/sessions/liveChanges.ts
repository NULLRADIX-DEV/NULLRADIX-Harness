import { useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { sessionKeys } from './queries';

// Reading changes starts several git processes, so they load again at most this often.
export const CHANGES_REFRESH_MS = 10_000;

interface Seen {
  sessionId: string;
  size: number;
  at: number;
}

// Changes and open diffs read again when the transcript of the session grew, throttled.
export function useRefreshChangesOnGrowth(sessionId: string, sizeBytes: number | null | undefined) {
  const queryClient = useQueryClient();
  const seen = useRef<Seen | null>(null);
  useEffect(() => {
    if (sizeBytes === null || sizeBytes === undefined) return;
    const last = seen.current;
    if (last?.sessionId !== sessionId) {
      seen.current = { sessionId, size: sizeBytes, at: 0 };
      return;
    }
    if (last.size === sizeBytes) return;
    const wait = Math.max(0, last.at + CHANGES_REFRESH_MS - Date.now());
    const timer = setTimeout(() => {
      seen.current = { sessionId, size: sizeBytes, at: Date.now() };
      void queryClient.invalidateQueries({ queryKey: sessionKeys.changes(sessionId) });
      void queryClient.invalidateQueries({ queryKey: sessionKeys.diffs(sessionId) });
    }, wait);
    return () => {
      clearTimeout(timer);
    };
  }, [queryClient, sessionId, sizeBytes]);
}
