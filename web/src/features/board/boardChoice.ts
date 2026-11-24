import type { Board } from '../boards/queries';

export const BOARD_STORAGE_KEY = 'nullradix-board';

export function readBoardChoice(storage: Pick<Storage, 'getItem'>): string | null {
  try {
    return storage.getItem(BOARD_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function saveBoardChoice(storage: Pick<Storage, 'setItem'>, boardId: string): void {
  try {
    storage.setItem(BOARD_STORAGE_KEY, boardId);
  } catch {
    // Blocked storage only forgets the choice on the next visit.
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
