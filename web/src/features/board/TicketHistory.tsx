import { useNow } from '../../shared/lib/useNow';
import type { HistoryLine } from '../tickets/queries';
import { historyView } from './history';

const DAY = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'long' });
const DAY_WITH_YEAR = new Intl.DateTimeFormat(undefined, {
  day: 'numeric',
  month: 'long',
  year: 'numeric',
});
const TIME = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });
const SEPARATOR = ` ${String.fromCharCode(0xb7)} `;
const FULL = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'medium' });

function startOfDay(at: number): number {
  const date = new Date(at);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

// Today, Yesterday or the date, with the year when it is not the current one.
export function dayLabel(at: number, now: number): string {
  const days = Math.round((startOfDay(now) - startOfDay(at)) / 86_400_000);
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  const sameYear = new Date(at).getFullYear() === new Date(now).getFullYear();
  return (sameYear ? DAY : DAY_WITH_YEAR).format(at);
}

interface Day {
  label: string;
  lines: HistoryLine[];
}

function byDay(lines: readonly HistoryLine[], now: number): Day[] {
  const days: Day[] = [];
  for (const line of lines) {
    const label = dayLabel(line.at, now);
    const last = days.at(-1);
    if (last?.label === label) last.lines.push(line);
    else days.push({ label, lines: [line] });
  }
  return days;
}

// History of a ticket, newest first and grouped by day.
export function TicketHistory({ history }: { history: readonly HistoryLine[] }) {
  const now = useNow(false);
  if (history.length === 0) return <p className="muted">No history yet.</p>;
  const newestFirst = [...history].reverse();
  return (
    <div className="tl">
      {byDay(newestFirst, now).map((day, index) => (
        <section key={`${day.label}-${String(index)}`} aria-label={day.label} className="tl-day">
          <h4 className="tl-day-title">{day.label}</h4>
          <ol className="tl-list">
            {day.lines.map((line, position) => {
              const view = historyView(line);
              return (
                <li
                  key={`${String(line.at)}-${line.field}-${String(position)}`}
                  className="tl-item"
                >
                  <span className={`tl-dot tone-${view.tone}`} aria-hidden="true" />
                  <div className={view.highlight ? 'tl-body highlight' : 'tl-body'}>
                    <div className="tl-head">
                      <span className="tl-title">{view.title}</span>
                      <time
                        className="tl-time"
                        dateTime={new Date(line.at).toISOString()}
                        title={FULL.format(line.at)}
                      >
                        {TIME.format(line.at)}
                      </time>
                    </div>
                    <div className="tl-meta">{view.meta.join(SEPARATOR)}</div>
                    {view.error && <div className="tl-error">{view.error}</div>}
                    {view.detail && <div className="tl-detail">{view.detail}</div>}
                  </div>
                </li>
              );
            })}
          </ol>
        </section>
      ))}
    </div>
  );
}
