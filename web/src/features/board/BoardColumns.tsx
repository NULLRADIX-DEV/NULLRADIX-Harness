import { useQuery } from '@tanstack/react-query';
import type { Board } from '../boards/queries';
import { COLUMNS, COLUMN_LABELS, byColumn } from '../tickets/labels';
import { ticketsQuery, type Column, type Ticket } from '../tickets/queries';
import { TicketCard } from './TicketCard';

export const WAITING_HINT =
  'Waiting follows from open dependencies; it changes when they are done.';

function BoardColumn({
  column,
  tickets,
  onOpen,
}: {
  column: Column;
  tickets: readonly Ticket[];
  onOpen: (key: string) => void;
}) {
  const locked = column === 'waiting';
  return (
    <section
      className={locked ? 'column db-column locked' : 'column db-column'}
      aria-label={COLUMN_LABELS[column]}
      data-status={column}
      title={locked ? WAITING_HINT : undefined}
    >
      <div className="column-head">
        <span className="column-dot" aria-hidden="true" />
        <h2>{COLUMN_LABELS[column]}</h2>
        <span className="count">{tickets.length}</span>
      </div>
      {tickets.length === 0 && <div className="column-empty">No tickets</div>}
      {tickets.map((ticket) => (
        <TicketCard key={ticket.key} ticket={ticket} onOpen={onOpen} />
      ))}
    </section>
  );
}

// The eight columns of a board with the cards in the order the daemon returns them.
export function BoardColumns({ board, onOpen }: { board: Board; onOpen: (key: string) => void }) {
  const { status, data } = useQuery(ticketsQuery(board.id));
  if (status === 'pending') return <p className="muted">Loading tickets...</p>;
  if (status === 'error') return <p className="form-error">Could not load the tickets.</p>;
  const columns = byColumn(data);
  return (
    <div className="columns db-columns">
      {COLUMNS.map((column) => (
        <BoardColumn
          key={column}
          column={column}
          tickets={columns.get(column) ?? []}
          onOpen={onOpen}
        />
      ))}
    </div>
  );
}
