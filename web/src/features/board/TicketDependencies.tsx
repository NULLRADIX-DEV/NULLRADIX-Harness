import type { Ticket } from '../tickets/queries';

function DependencyRow({
  label,
  keys,
  open,
  onOpen,
}: {
  label: string;
  keys: readonly string[];
  open?: ReadonlySet<string>;
  onOpen: (key: string) => void;
}) {
  if (keys.length === 0) return null;
  return (
    <div className="dep-row" role="group" aria-label={label}>
      <span className="dep-label">{label}</span>
      {keys.map((key) => (
        <button
          key={key}
          type="button"
          className={open?.has(key) ? 'dep is-open' : 'dep'}
          title={open && (open.has(key) ? `${key} is still open` : `${key} is finished`)}
          onClick={() => {
            onOpen(key);
          }}
        >
          {key}
        </button>
      ))}
    </div>
  );
}

export function TicketDependencies({
  ticket,
  onOpen,
}: {
  ticket: Ticket;
  onOpen: (key: string) => void;
}) {
  const open = new Set(ticket.dependsOpen);
  const none =
    ticket.dependsOn.length === 0 && ticket.dependents.length === 0 && ticket.unlocks.length === 0;
  return (
    <section className="db-side-section" aria-label="Dependencies">
      <h3 className="db-side-title">Dependencies</h3>
      {none ? (
        <p className="muted">None.</p>
      ) : (
        <div className="db-deps">
          <DependencyRow label="Depends on" keys={ticket.dependsOn} open={open} onOpen={onOpen} />
          <DependencyRow label="Needed by" keys={ticket.dependents} onOpen={onOpen} />
          <DependencyRow label="Ready once this is done" keys={ticket.unlocks} onOpen={onOpen} />
        </div>
      )}
    </section>
  );
}
