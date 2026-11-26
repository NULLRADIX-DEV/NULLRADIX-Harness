import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import { problemMessage, type ProblemMessages } from '../../shared/api/problemMessage';
import { useInvalidate } from '../../shared/query/useInvalidate';
import { isActive, runKeys, type Run } from '../runs/queries';
import { ticketKeys, type Ticket } from '../tickets/queries';

const MESSAGES: ProblemMessages = {
  'plan-not-possible':
    'This ticket cannot be planned now: it is being worked on, done or not needed.',
  'plan-waiting-question': 'An open question holds this ticket back; decide it first.',
  'plan-running': 'A planning of this ticket is running already.',
  'plan-claimed': 'Another agent is planning this ticket right now.',
  'plan-no-repo':
    'Set the repository of the board of this ticket on this machine, then plan it again.',
  'dispatch-paused': 'The dispatcher is paused or no Claude account is free. Try again later.',
  'plan-worktree-busy':
    'A folder of an earlier planning is in the way. Check the folder next to the repository by hand.',
  'ticket-not-found': 'This ticket no longer exists.',
};

// Plan runs of one ticket, newest first; run changes refetch them through the runs keys.
function planRunsQuery(key: string) {
  return {
    queryKey: [...runKeys.all, 'ticket', key, 'plan'] as const,
    queryFn: async (): Promise<Run[]> =>
      unwrap(await api.GET('/api/v1/runs', { params: { query: { ticketKey: key, kind: 'plan' } } }))
        .items,
  };
}

// A plan run is under way, or ended and waits for the dispatcher to settle it.
function isPlanning(runs: readonly Run[]): boolean {
  return runs.some((run) => isActive(run.status) || run.result?.outcome == null);
}

function useStartPlanning() {
  const tickets = useInvalidate(ticketKeys.all);
  const runs = useInvalidate(runKeys.all);
  return useMutation({
    mutationFn: async (key: string) =>
      unwrap(
        await api.POST('/api/v1/tickets/{ticketKey}/plan-runs', {
          params: { path: { ticketKey: key } },
        }),
      ),
    onSettled: async () => {
      await Promise.all([tickets(), runs()]);
    },
  });
}

const NOT_PLANNED = new Set(['in-progress', 'done', 'not-needed']);

// Starts a planning run by hand; the dispatcher settles it and the plan arrives as live updates.
export function TicketPlanning({ ticket }: { ticket: Ticket }) {
  const runs = useQuery(planRunsQuery(ticket.key));
  const start = useStartPlanning();
  if (NOT_PLANNED.has(ticket.status)) return null;
  const planning = runs.data !== undefined && isPlanning(runs.data);
  const error = problemMessage(start.error, MESSAGES, 'Could not start the planning.');
  return (
    <section className="db-side-section" aria-label="Planning">
      <h3 className="db-side-title">Planning</h3>
      <div className="db-planning">
        <button
          type="button"
          className="btn small"
          disabled={runs.data === undefined || planning || start.isPending}
          onClick={() => {
            start.mutate(ticket.key);
          }}
        >
          {planning || start.isPending ? 'Planning...' : 'Plan'}
        </button>
        {error !== null && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
      </div>
    </section>
  );
}
