import type { BoardRef, BoardsState } from '@nullradix/theme-sdk';
import { useQuery } from '@tanstack/react-query';
import { useCallback, useMemo } from 'react';
import { pickBoard } from '../../features/board/boardChoice';
import { chooseBoardId, useBoardChoice } from '../../features/board/useBoardChoice';
import { boardsQuery } from '../../features/boards/queries';
import { projectsQuery } from '../../features/projects/queries';

// Projects, boards and the board choice that the board page and the switcher share.
export function useHostBoards(active: boolean): BoardsState {
  const boards = useQuery({ ...boardsQuery(), enabled: active });
  const projects = useQuery({ ...projectsQuery(), enabled: active });
  const choice = useBoardChoice();
  const list = boards.data;
  const current = list ? pickBoard(list, choice) : undefined;

  // a project alone means its first board; an unknown board or project changes nothing
  const choose = useCallback(
    (ref: BoardRef) => {
      const target = list?.find(
        (board) =>
          board.projectKey === ref.project && (ref.board === null || board.id === ref.board),
      );
      if (target) chooseBoardId(target.id);
    },
    [list],
  );

  return useMemo(
    () => ({
      current: current ? { project: current.projectKey, board: current.id } : null,
      projects: (projects.data ?? []).map(({ key, name }) => ({ id: key, name })),
      boards: (list ?? []).map(({ id, projectKey, name }) => ({ id, project: projectKey, name })),
      loading: active && (boards.isPending || projects.isPending),
      choose,
    }),
    [active, current, projects.data, projects.isPending, list, boards.isPending, choose],
  );
}
