import { useSyncExternalStore } from 'react';
import { BOARD_STORAGE_KEY, readBoardChoice, saveBoardChoice } from './boardChoice';

const listeners = new Set<() => void>();

// The choice of this visit while the storage is blocked.
let unsaved: string | null = null;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  // another tab chose a board
  const onStorage = (event: StorageEvent) => {
    if (event.key === BOARD_STORAGE_KEY) listener();
  };
  window.addEventListener('storage', onStorage);
  return () => {
    listeners.delete(listener);
    window.removeEventListener('storage', onStorage);
  };
}

// Remembers the board for every page and the board switcher.
export function chooseBoardId(boardId: string): void {
  unsaved = saveBoardChoice(window.localStorage, boardId) ? null : boardId;
  for (const listener of listeners) listener();
}

// The chosen board id, null before a choice; renders again when it changes.
export function useBoardChoice(): string | null {
  return useSyncExternalStore(subscribe, () => unsaved ?? readBoardChoice(window.localStorage));
}
