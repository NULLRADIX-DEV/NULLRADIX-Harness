import { useQuery } from '@tanstack/react-query';
import { ticketRunsQuery } from './dispatchQueries';
import { isActive } from './queries';
import { RunView } from './RunView';

// Runs of one ticket, newest first; the newest and active ones start open.
export function TicketRuns({ ticketKey }: { ticketKey: string }) {
  const { status, data } = useQuery(ticketRunsQuery(ticketKey));
  if (status === 'pending') return <p className="muted">Loading runs...</p>;
  if (status === 'error') return <p className="form-error">Could not load the runs.</p>;
  if (data.length === 0) return <p className="muted">No runs for this ticket yet.</p>;
  return (
    <div className="run-cards">
      {data.map((run, index) => (
        <RunView key={run.id} run={run} defaultOpen={index === 0 || isActive(run.status)} />
      ))}
    </div>
  );
}
