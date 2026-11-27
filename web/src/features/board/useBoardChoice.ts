import { useSyncExternalStore } from 'react';
import { localBoardStorage, readBoardChoice, saveBoardChoice } from './boardChoice';

const listeners = new Set<() => void>();

// The choice of this visit while the storage is blocked.
let unsaved: string | null = null;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// Remembers the board for every page and the board switcher.
export function chooseBoardId(boardId: string): void {
  const storage = localBoardStorage();
  unsaved = storage && saveBoardChoice(storage, boardId) ? null : boardId;
  for (const listener of listeners) listener();
}

// Forgets a choice kept only for this visit; tests call it between cases.
export function forgetUnsavedBoardChoice(): void {
  unsaved = null;
}

function snapshot(): string | null {
  const storage = localBoardStorage();
  return unsaved ?? (storage ? readBoardChoice(storage) : null);
}

// The chosen board id, null before a choice; renders again when it changes.
export function useBoardChoice(): string | null {
  return useSyncExternalStore(subscribe, snapshot);
}
