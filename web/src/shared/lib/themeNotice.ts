import { useSyncExternalStore } from 'react';

// A note about the theme: an error of a broken theme, or information such as safe mode.
export interface ThemeNoticeEntry {
  message: string;
  kind: 'error' | 'info';
}

type Listener = () => void;

let notice: ThemeNoticeEntry | undefined;
const listeners = new Set<Listener>();

// Sets the note (a text is an error, undefined removes it).
export function setThemeNotice(value: string | ThemeNoticeEntry | undefined): void {
  notice = typeof value === 'string' ? { message: value, kind: 'error' } : value;
  for (const listener of listeners) listener();
}

export function getThemeNotice(): ThemeNoticeEntry | undefined {
  return notice;
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// The current note; renders again on every change.
export function useThemeNotice(): ThemeNoticeEntry | undefined {
  return useSyncExternalStore(subscribe, getThemeNotice);
}
