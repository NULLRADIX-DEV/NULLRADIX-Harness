import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router';
import { boardsQuery } from '../boards/queries';
import { BoardColumns } from './BoardColumns';
import { BoardPicker } from './BoardPicker';
import { pickBoard, readBoardChoice, saveBoardChoice } from './boardChoice';
import { NewTicketDialog } from './NewTicketDialog';
import { TicketPanel } from './TicketPanel';
import './board.css';

export const NOTICE_MS = 4000;
export const TICKET_PARAM = 'ticket';

function NoBoard() {
  return (
    <div className="state empty">
      <h2>No board yet</h2>
      <p>
        Create a project under Settings, then a board with{' '}
        <code className="mono">nullradix board create KEY NAME</code>.
      </p>
    </div>
  );
}

export function BoardPage() {
  const boards = useQuery(boardsQuery());
  const [choice, setChoice] = useState(() => readBoardChoice(window.localStorage));
  const [searchParams, setSearchParams] = useSearchParams();
  const [creating, setCreating] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const board = boards.data ? pickBoard(boards.data, choice) : undefined;
  const openKey = searchParams.get(TICKET_PARAM);

  useEffect(() => {
    if (notice === null) return;
    const timer = setTimeout(() => {
      setNotice(null);
    }, NOTICE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [notice]);

  function openTicket(key: string) {
    setSearchParams({ [TICKET_PARAM]: key });
  }

  function closeTicket() {
    setSearchParams({});
  }

  function chooseBoard(boardId: string) {
    setChoice(boardId);
    saveBoardChoice(window.localStorage, boardId);
    if (openKey !== null) closeTicket();
  }

  return (
    <div className="page db-board">
      <div className="db-board-head">
        <h1 className="page-title">Board</h1>
        {board && boards.data && (
          <BoardPicker boards={boards.data} value={board.id} onChange={chooseBoard} />
        )}
        <button
          type="button"
          className="btn primary db-board-new"
          disabled={!board}
          onClick={() => {
            setNotice(null);
            setCreating(true);
          }}
        >
          New ticket
        </button>
      </div>
      {notice !== null && (
        <div className="banner info" role="status">
          {notice}
        </div>
      )}
      {boards.status === 'pending' && <p className="muted">Loading boards...</p>}
      {boards.status === 'error' && <p className="form-error">Could not load the boards.</p>}
      {boards.data?.length === 0 && <NoBoard />}
      {board && <BoardColumns board={board} onOpen={openTicket} />}
      {creating && board && (
        <NewTicketDialog
          board={board}
          onClose={() => {
            setCreating(false);
          }}
          onCreated={(ticket) => {
            setCreating(false);
            setNotice(`${ticket.key} created.`);
          }}
        />
      )}
      {openKey !== null && (
        <TicketPanel key={openKey} ticketKey={openKey} onClose={closeTicket} onOpen={openTicket} />
      )}
    </div>
  );
}
