import { useSyncExternalStore } from 'react';

export const TICK_MS = 1000;

const listeners = new Set<() => void>();
let now = Date.now();
let timer: ReturnType<typeof setInterval> | undefined;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (timer === undefined) {
    now = Date.now();
    timer = setInterval(() => {
      now = Date.now();
      for (const notify of listeners) notify();
    }, TICK_MS);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

function stay(): () => void {
  return () => undefined;
}

function snapshot(): number {
  return now;
}

// The current time in milliseconds, renewed every second while `ticking`.
export function useNow(ticking: boolean): number {
  return useSyncExternalStore(ticking ? subscribe : stay, snapshot);
}
