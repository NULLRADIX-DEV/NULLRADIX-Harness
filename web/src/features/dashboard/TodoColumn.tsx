import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { Link } from 'react-router';
import { FormError } from '../../shared/ui/FormError';
import { inboxTarget } from '../inbox/InboxPage';
import { inboxQuery, useInboxAction, type InboxItem, type InboxKind } from '../inbox/queries';
import { waitText } from './format';

// From this age a blocked entry counts as urgent.
export const URGENT_MS = 24 * 3_600_000;

type GroupKey = 'blocked' | 'question' | 'accept' | 'note';

const GROUPS: readonly { key: GroupKey; label: string; kinds: readonly InboxKind[] }[] = [
  { key: 'blocked', label: 'Blocked', kinds: ['merge', 'setup', 'blocked'] },
  { key: 'question', label: 'Questions', kinds: ['question'] },
  { key: 'accept', label: 'Approvals', kinds: ['accept', 'planning'] },
  { key: 'note', label: 'Notes', kinds: ['note'] },
];

function shownKey(item: InboxItem): string {
  return item.type === 'note' ? `N-${item.key}` : item.key;
}

function TodoRow({ item, now, flag }: { item: InboxItem; now: number; flag: boolean }) {
  const action = useInboxAction();
  const urgent = flag && now - item.sinceAt >= URGENT_MS;
  const waiting =
    item.kind === 'question' && item.waitingTickets > 0
      ? `${String(item.waitingTickets)} ${item.waitingTickets === 1 ? 'ticket waits' : 'tickets wait'}`
      : null;
  const detail = [`since ${waitText(item.sinceAt, now)}`, item.detail ?? waiting]
    .filter(Boolean)
    .join(' / ');
  const label = item.kind === 'accept' ? 'Accept' : item.kind === 'note' ? 'Make ticket' : null;
  return (
    <li
      className={`todo-row${flag ? ' flagged' : ''}${urgent ? ' urgent' : ''}`}
      data-key={`${item.type}:${item.key}`}
    >
      {flag && (
        <span
          className="todo-urgent"
          role={urgent ? 'img' : undefined}
          aria-label={urgent ? 'Waiting for over 24 hours' : undefined}
        >
          {urgent ? '!' : ''}
        </span>
      )}
      <span className="mono todo-id">{shownKey(item)}</span>
      <div className="todo-main">
        <Link to={inboxTarget(item)} className="todo-title">
          {item.title}
        </Link>
        <span className="muted todo-detail">{detail}</span>
        {action.error && <FormError message={action.error.message} />}
      </div>
      {label ? (
        <button
          type="button"
          className="todo-action"
          aria-label={`${label} ${shownKey(item)}`}
          disabled={action.isPending}
          onClick={() => {
            action.mutate(item);
          }}
        >
          {label}
        </button>
      ) : (
        <Link
          to={inboxTarget(item)}
          className="todo-action"
          aria-label={`${item.kind === 'question' ? 'Answer' : 'Open'} ${shownKey(item)}`}
        >
          {item.kind === 'question' ? 'Answer' : 'Open'}
        </Link>
      )}
    </li>
  );
}

// What waits for the person, in four groups that open when they hold entries.
export function TodoColumn({ boardId, now }: { boardId: string | null; now: number }) {
  const inbox = useQuery(inboxQuery(boardId));
  // groups opened or closed by hand; otherwise those with entries are open
  const [toggled, setToggled] = useState<Partial<Record<GroupKey, boolean>>>({});
  const items = inbox.data?.items ?? [];
  return (
    <aside className="todo-column" aria-label="What do I have to do?">
      <div className="todo-scroll">
        {inbox.status === 'pending' && <p className="muted todo-note">Loading tasks...</p>}
        {inbox.status === 'error' && (
          <p className="form-error todo-note" role="alert">
            Could not load the tasks.
          </p>
        )}
        {inbox.data && items.length === 0 && (
          <p className="todo-done">Nothing to do - the dispatcher gets on without you.</p>
        )}
        {inbox.data &&
          items.length > 0 &&
          GROUPS.map((group) => {
            const rows = items
              .filter((item) => group.kinds.includes(item.kind))
              .sort(
                (a, b) => (group.key === 'blocked' ? a.sinceAt - b.sinceAt : 0) || a.rank - b.rank,
              );
            const open = toggled[group.key] ?? rows.length > 0;
            const urgent =
              group.key === 'blocked'
                ? rows.filter((row) => now - row.sinceAt >= URGENT_MS).length
                : 0;
            return (
              <section key={group.key} className={`todo-group${rows.length > 0 ? '' : ' empty'}`}>
                <button
                  type="button"
                  className="todo-head"
                  aria-expanded={open}
                  onClick={() => {
                    setToggled((current) => ({ ...current, [group.key]: !open }));
                  }}
                >
                  {!open && urgent > 0 && (
                    <>
                      <span
                        className="todo-urgent"
                        aria-hidden="true"
                        title={`${String(urgent)} waiting for over 24 hours`}
                      >
                        !
                      </span>
                      <span className="visually-hidden">{`${String(urgent)} waiting for over 24 hours`}</span>
                    </>
                  )}
                  <span className="todo-label">{group.label}</span>
                  <span className="mono todo-count">{rows.length}</span>
                  <span className="todo-chevron" aria-hidden="true">
                    &#9662;
                  </span>
                </button>
                {open &&
                  (rows.length > 0 ? (
                    <ul aria-label={group.label}>
                      {rows.map((item) => (
                        <TodoRow
                          key={`${item.type}:${item.key}`}
                          item={item}
                          now={now}
                          flag={group.key === 'blocked'}
                        />
                      ))}
                    </ul>
                  ) : (
                    <p className="todo-none">Nothing open.</p>
                  ))}
              </section>
            );
          })}
      </div>
    </aside>
  );
}
