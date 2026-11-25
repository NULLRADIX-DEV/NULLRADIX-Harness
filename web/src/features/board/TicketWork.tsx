import type { Ticket } from '../tickets/queries';

// Where the work on a ticket happens; read only, as in legacy.
export function TicketWork({ ticket }: { ticket: Ticket }) {
  const rows: [string, string | null | undefined][] = [
    ['Target branch', ticket.targetBranch],
    ['Branch', ticket.branch],
    ['Worktree', ticket.worktree],
  ];
  const shown = rows.filter((row): row is [string, string] => Boolean(row[1]));
  if (shown.length === 0) return null;
  return (
    <section className="db-side-section" aria-label="Work">
      <h3 className="db-side-title">Work</h3>
      <dl className="db-work-list">
        {shown.map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd className="mono">{value}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
