import type { Board } from '../boards/queries';

export const BOARD_STORAGE_KEY = 'nullradix-board';

// The local storage, or null where even reaching it fails.
export function localBoardStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export function readBoardChoice(storage: Pick<Storage, 'getItem'>): string | null {
  try {
    return storage.getItem(BOARD_STORAGE_KEY);
  } catch {
    return null;
  }
}

// False when the storage is blocked; the choice then lasts only for this visit.
export function saveBoardChoice(storage: Pick<Storage, 'setItem'>, boardId: string): boolean {
  try {
    storage.setItem(BOARD_STORAGE_KEY, boardId);
    return true;
  } catch {
    return false;
  }
}

export function pickBoard(boards: readonly Board[], choice: string | null): Board | undefined {
  return boards.find((board) => board.id === choice) ?? boards[0];
}

export interface ProjectBoards {
  projectKey: string;
  boards: Board[];
}

export function groupByProject(boards: readonly Board[]): ProjectBoards[] {
  const groups: ProjectBoards[] = [];
  for (const board of boards) {
    const group = groups.find((entry) => entry.projectKey === board.projectKey);
    if (group) group.boards.push(board);
    else groups.push({ projectKey: board.projectKey, boards: [board] });
  }
  return groups;
}
