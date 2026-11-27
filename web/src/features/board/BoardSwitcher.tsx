import { useQuery } from '@tanstack/react-query';
import { boardsQuery } from '../boards/queries';
import { BoardPicker } from './BoardPicker';
import { pickBoard } from './boardChoice';
import { chooseBoardId, useBoardChoice } from './useBoardChoice';

// The board choice for a theme shell; the board page and the questions follow it.
export function BoardSwitcher() {
  const boards = useQuery(boardsQuery());
  const choice = useBoardChoice();
  const board = boards.data ? pickBoard(boards.data, choice) : undefined;
  if (!boards.data || !board) return null;
  return <BoardPicker boards={boards.data} value={board.id} onChange={chooseBoardId} />;
}
