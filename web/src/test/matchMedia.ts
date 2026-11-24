import { vi } from 'vitest';

export interface MediaControl {
  setMatches: (matches: boolean) => void;
}

// jsdom has no matchMedia; this one reports the given result for every query.
export function mockMatchMedia(initial: boolean): MediaControl {
  let matches = initial;
  const listeners = new Set<(event: MediaQueryListEvent) => void>();
  vi.stubGlobal('matchMedia', (query: string) => ({
    get matches() {
      return matches;
    },
    media: query,
    addEventListener: (_type: string, listener: (event: MediaQueryListEvent) => void) => {
      listeners.add(listener);
    },
    removeEventListener: (_type: string, listener: (event: MediaQueryListEvent) => void) => {
      listeners.delete(listener);
    },
  }));
  return {
    setMatches: (next) => {
      matches = next;
      for (const listener of listeners) listener({ matches: next } as MediaQueryListEvent);
    },
  };
}
