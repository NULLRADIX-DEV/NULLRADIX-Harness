import { useQuery } from '@tanstack/react-query';
import { Link } from 'react-router';
import { formatAgo, sessionTitle, useLiveSessions } from '../sessions/format';
import { ticketSessionsQuery } from '../sessions/queries';
import { SessionStateBadge } from '../sessions/SessionStateBadge';

// Sessions that worked on the ticket; hidden while there are none.
export function TicketSessions({ ticketKey }: { ticketKey: string }) {
  const { data } = useQuery(ticketSessionsQuery(ticketKey));
  const { sessions, now } = useLiveSessions(data);
  if (sessions.length === 0) return null;
  return (
    <section className="db-side-section" aria-label="Sessions">
      <h3 className="db-side-title">Sessions</h3>
      <ul className="db-ticket-sessions">
        {sessions.map((session) => (
          <li key={session.id}>
            <SessionStateBadge state={session.displayState} />
            <Link
              className="db-ticket-session-name"
              title={session.id}
              to={`/sessions/${encodeURIComponent(session.id)}`}
            >
              {sessionTitle(session)}
            </Link>
            <span className="muted">{formatAgo(session.lastEventAt, now)}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
