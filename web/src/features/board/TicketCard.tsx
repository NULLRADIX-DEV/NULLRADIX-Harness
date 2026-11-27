import { Card } from '../../shared/ui/Card';
import { Progress } from '../../shared/ui/Progress';
import { PRIORITY_LABELS } from '../tickets/labels';
import type { Ticket } from '../tickets/queries';

function StepsProgress({ done, total }: { done: number; total: number }) {
  const complete = done >= total;
  return (
    <Progress done={done} total={total} label="Steps done" caption="steps done">
      <span className="card-steps">
        <span
          className="card-steps-bar"
          role="progressbar"
          aria-label="Steps done"
          aria-valuemin={0}
          aria-valuemax={total}
          aria-valuenow={done}
        >
          <span
            className={complete ? 'card-steps-fill complete' : 'card-steps-fill'}
            style={{ width: `${String(Math.round((done / total) * 100))}%` }}
          />
        </span>
        <span className="card-steps-text">
          {done}/{total} steps
        </span>
      </span>
    </Progress>
  );
}

export function TicketCard({ ticket, onOpen }: { ticket: Ticket; onOpen: (key: string) => void }) {
  const blocked = ticket.status === 'blocked';
  return (
    <Card as="article" className={ticket.waiting ? 'card db-card is-waiting' : 'card db-card'}>
      <button
        type="button"
        className="db-card-open"
        onClick={() => {
          onOpen(ticket.key);
        }}
      >
        <span className="card-top">
          <span className="card-id">{ticket.key}</span>
          <span className="chips end">
            {ticket.priority !== 'normal' && (
              <span className={`badge prio-${ticket.priority}`}>
                {PRIORITY_LABELS[ticket.priority]}
              </span>
            )}
            {blocked && (
              <span className="badge blocked" title={ticket.blockedReason ?? undefined}>
                Blocked
              </span>
            )}
          </span>
        </span>
        <span className="card-title">{ticket.title}</span>
        <span className="chips">
          {ticket.assignee && <span className="badge">@{ticket.assignee}</span>}
          {ticket.needsPlanning && <span className="badge waiting">Planning</span>}
          {ticket.openQuestions.length > 0 && (
            <span
              className="badge waiting"
              title={ticket.openQuestions.map((question) => question.key).join(', ')}
            >
              Question
            </span>
          )}
          {ticket.plan.trim() !== '' && <span className="badge">Plan</span>}
          {ticket.handbook.trim() !== '' && <span className="badge">Handbook</span>}
          {ticket.diagram.trim() !== '' && <span className="badge">Diagram</span>}
          {ticket.labels.map((label) => (
            <span key={label} className="tag">
              {label}
            </span>
          ))}
        </span>
        {ticket.stepsTotal > 0 && (
          <StepsProgress done={ticket.stepsDone} total={ticket.stepsTotal} />
        )}
        {ticket.waiting && (
          <span className="card-waiting">waits on {ticket.dependsOpen.join(', ')}</span>
        )}
      </button>
    </Card>
  );
}
