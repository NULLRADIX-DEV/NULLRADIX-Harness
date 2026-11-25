import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchTranscript, type TranscriptEntry } from './queries';

// Entries per page; a full page means there is probably more.
export const TRANSCRIPT_PAGE_SIZE = 500;

export interface TranscriptState {
  status: 'loading' | 'ready' | 'error';
  entries: TranscriptEntry[];
  available: boolean;
  // Broken lines over all pages.
  skipped: number;
  // Every page there is has been read; later pages come with live refreshes.
  complete: boolean;
}

const INITIAL: TranscriptState = {
  status: 'loading',
  entries: [],
  available: true,
  skipped: 0,
  complete: false,
};

// Appends new entries; a known id replaces the earlier entry in its place, as a finished tool call.
export function mergeEntries(
  current: TranscriptEntry[],
  incoming: TranscriptEntry[],
): TranscriptEntry[] {
  if (incoming.length === 0) return current;
  const index = new Map(current.map((entry, position) => [entry.id, position]));
  const next = [...current];
  for (const entry of incoming) {
    const position = index.get(entry.id);
    if (position === undefined) {
      index.set(entry.id, next.length);
      next.push(entry);
    } else {
      next[position] = entry;
    }
  }
  return next;
}

interface Pull {
  cursor: number;
  busy: boolean;
  again: boolean;
}

const freshPull = (): Pull => ({ cursor: 0, busy: false, again: false });

// Reads the transcript in pages and reads on when `version` changes; key the user by session.
export function useTranscript(sessionId: string, version: number) {
  const [state, setState] = useState<TranscriptState>(INITIAL);
  const pull = useRef<Pull>(freshPull());

  const read = useCallback(async () => {
    const own = pull.current;
    if (own.busy) {
      own.again = true;
      return;
    }
    own.busy = true;
    // read through a function so the check sees writes made while awaiting
    const again = () => own.again;
    try {
      do {
        own.again = false;
        for (;;) {
          const page = await fetchTranscript(sessionId, own.cursor, TRANSCRIPT_PAGE_SIZE);
          if (own !== pull.current) return;
          own.cursor = page.cursor;
          const full = page.available && page.entries.length >= TRANSCRIPT_PAGE_SIZE;
          setState((previous) => ({
            status: 'ready',
            entries: mergeEntries(previous.entries, page.entries),
            available: page.available,
            skipped: previous.skipped + page.skipped,
            complete: !full,
          }));
          if (!full) break;
        }
      } while (again());
    } catch {
      if (own !== pull.current) return;
      // entries already shown stay; the next refresh tries again
      setState((previous) =>
        previous.status === 'ready'
          ? { ...previous, complete: true }
          : { ...INITIAL, status: 'error' },
      );
    } finally {
      own.busy = false;
    }
  }, [sessionId]);

  // a page of an earlier session or a closed view must not land
  useEffect(
    () => () => {
      pull.current = freshPull();
    },
    [sessionId],
  );

  useEffect(() => {
    void read();
  }, [read, version]);

  const retry = useCallback(() => {
    pull.current = freshPull();
    setState(INITIAL);
    void read();
  }, [read]);

  return { transcript: state, retry };
}
