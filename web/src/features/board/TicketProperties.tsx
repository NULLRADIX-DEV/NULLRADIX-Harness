import {
  PRIORITIES,
  PRIORITY_LABELS,
  STATUSES,
  COLUMN_LABELS,
  isPriority,
  isStatus,
  splitList,
} from '../tickets/labels';
import type { Ticket, TicketStatus } from '../tickets/queries';
import { InlineField } from './InlineField';
import type { TicketActions } from './useTicketActions';

function labelText(text: string): string {
  return splitList(text).join(', ');
}

function orNull(text: string): string | null {
  const value = text.trim();
  return value === '' ? null : value;
}

export function TicketProperties({
  ticket,
  actions,
  onStatus,
}: {
  ticket: Ticket;
  actions: TicketActions;
  onStatus: (status: TicketStatus) => void;
}) {
  const { busy } = actions;
  return (
    <section className="db-properties-grid" aria-label="Properties">
      <div className="db-fields">
        <label className="field">
          <span>Status</span>
          <select
            value={ticket.status}
            aria-disabled={busy}
            onChange={(event) => {
              if (!busy && isStatus(event.target.value)) onStatus(event.target.value);
            }}
          >
            {STATUSES.map((status) => (
              <option key={status} value={status}>
                {COLUMN_LABELS[status]}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>Priority</span>
          <select
            value={ticket.priority}
            aria-disabled={busy}
            onChange={(event) => {
              if (!busy && isPriority(event.target.value))
                void actions.save({ priority: event.target.value });
            }}
          >
            {PRIORITIES.map((priority) => (
              <option key={priority} value={priority}>
                {PRIORITY_LABELS[priority]}
              </option>
            ))}
          </select>
        </label>
        <label className="option-row db-field-check">
          <input
            type="checkbox"
            checked={ticket.needsPlanning}
            aria-disabled={busy}
            onChange={(event) => {
              if (busy) return;
              void actions.save({ needsPlanning: event.target.checked });
            }}
          />
          <span>Needs planning</span>
        </label>
        <InlineField
          label="Assignee"
          value={ticket.assignee ?? ''}
          placeholder="Nobody"
          busy={busy}
          onConflict={actions.conflict}
          onSave={(text) => void actions.save({ assignee: orNull(text) })}
        />
        <InlineField
          label="Labels"
          value={ticket.labels.join(', ')}
          placeholder="comma separated"
          busy={busy}
          normalize={labelText}
          onConflict={actions.conflict}
          onSave={(text) => void actions.save({ labels: splitList(text) })}
        />
        <InlineField
          label="Target branch"
          value={ticket.targetBranch ?? ''}
          placeholder="Default branch"
          busy={busy}
          onConflict={actions.conflict}
          onSave={(text) => void actions.save({ targetBranch: orNull(text) })}
        />
        <InlineField
          label="Blocked reason"
          value={ticket.blockedReason ?? ''}
          placeholder="Not blocked"
          busy={busy}
          wide
          onConflict={actions.conflict}
          onSave={(text) => void actions.save({ blockedReason: orNull(text) })}
        />
      </div>
    </section>
  );
}
