import { useQuery } from '@tanstack/react-query';
import { useNow } from '../../shared/lib/useNow';
import { formatAgo, sessionTitle } from '../sessions/format';
import { ticketSessionsQuery } from '../sessions/queries';
import { SessionStateBadge } from '../sessions/SessionStateBadge';

// Sessions that worked on the ticket; hidden while there are none.
export function TicketSessions({ ticketKey }: { ticketKey: string }) {
  const { data } = useQuery(ticketSessionsQuery(ticketKey));
  const now = useNow(false);
  if (!data || data.length === 0) return null;
  return (
    <section className="db-side-section" aria-label="Sessions">
      <h3 className="db-side-title">Sessions</h3>
      <ul className="db-ticket-sessions">
        {data.map((session) => (
          <li key={session.id}>
            <SessionStateBadge state={session.displayState} />
            <span className="db-ticket-session-name" title={session.id}>
              {sessionTitle(session)}
            </span>
            <span className="muted">{formatAgo(session.lastEventAt, now)}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
