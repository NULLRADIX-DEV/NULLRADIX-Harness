import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router';
import { boardsQuery } from '../boards/queries';
import { DispatcherIndicator } from '../dispatcher/DispatcherIndicator';
import { NotesDialog } from '../notes/NotesDialog';
import { BoardColumns } from './BoardColumns';
import { BoardPicker } from './BoardPicker';
import { pickBoard, readBoardChoice, saveBoardChoice } from './boardChoice';
import { NewTicketDialog } from './NewTicketDialog';
import { TicketPanel } from './TicketPanel';
import './board.css';
import { Button } from '../../shared/ui/Button';

export const NOTICE_MS = 4000;
export const TICKET_PARAM = 'ticket';
// Opens the page on this board; read once, then dropped from the address.
export const BOARD_PARAM = 'board';
// Opens the notes dialog of the board; read once, then dropped from the address.
export const NOTES_PARAM = 'notes';

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
  const [searchParams, setSearchParams] = useSearchParams();
  const [choice, setChoice] = useState(
    () => searchParams.get(BOARD_PARAM) ?? readBoardChoice(window.localStorage),
  );
  const [creating, setCreating] = useState(false);
  const [noting, setNoting] = useState(() => searchParams.get(NOTES_PARAM) === '1');
  const [notice, setNotice] = useState<string | null>(null);
  const board = boards.data ? pickBoard(boards.data, choice) : undefined;
  const openKey = searchParams.get(TICKET_PARAM);
  const linkedBoard = searchParams.get(BOARD_PARAM);
  const linkedNotes = searchParams.has(NOTES_PARAM);

  // An inbox link chose the board and the dialog; keep the choice and clear the address.
  useEffect(() => {
    if (linkedBoard === null && !linkedNotes) return;
    if (linkedBoard !== null) saveBoardChoice(window.localStorage, linkedBoard);
    const next = new URLSearchParams(searchParams);
    next.delete(BOARD_PARAM);
    next.delete(NOTES_PARAM);
    setSearchParams(next, { replace: true });
  }, [linkedBoard, linkedNotes, searchParams, setSearchParams]);

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
        <div className="db-board-dispatcher">
          <DispatcherIndicator />
        </div>
        <Button
          type="button"
          disabled={!board}
          onClick={() => {
            setNoting(true);
          }}
        >
          Notes
        </Button>
        <Button
          type="button"
          variant="primary"
          disabled={!board}
          onClick={() => {
            setNotice(null);
            setCreating(true);
          }}
        >
          New ticket
        </Button>
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
      {noting && board && (
        <NotesDialog
          board={board}
          onClose={() => {
            setNoting(false);
          }}
          onOpenTicket={(key) => {
            setNoting(false);
            openTicket(key);
          }}
        />
      )}
      {openKey !== null && (
        <TicketPanel key={openKey} ticketKey={openKey} onClose={closeTicket} onOpen={openTicket} />
      )}
    </div>
  );
}
