import { useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { Link } from 'react-router';
import {
  DEFAULT_STATES,
  STATES,
  STATE_LABELS,
  formatAgo,
  formatDuration,
  groupByTicket,
  sessionRuntime,
  sessionTitle,
  summarize,
  useLiveSessions,
  type SessionGroup,
} from './format';
import { sessionsQuery, type DisplayState, type Session } from './queries';
import { FavoritesSection, FavoriteStar } from './Favorites';
import { SessionStateBadge } from './SessionStateBadge';
import { Panel } from '../../shared/ui/Panel';
import { Button } from '../../shared/ui/Button';

const SHORT_ID = 8;

function Subagents({ session, now }: { session: Session; now: number }) {
  const running = session.subagents.filter((agent) => agent.state === 'running');
  if (running.length === 0) return null;
  return (
    <ul className="session-agents" aria-label={`Subagents of ${sessionTitle(session)}`}>
      {running.map((agent) => (
        <li key={agent.id}>
          <SessionStateBadge state="running" />
          <span className="mono">{formatDuration(now - agent.startedAt)}</span>
          <span>
            Subagent <span className="mono">{agent.type}</span>
          </span>
        </li>
      ))}
    </ul>
  );
}

// One session: state, runtime, title, id, last activity, folder and running subagents.
function SessionRow({ session, now }: { session: Session; now: number }) {
  const short = session.id.slice(0, SHORT_ID);
  const dim = session.displayState === 'stale' || session.displayState === 'ended';
  return (
    <li className={dim ? 'session-row dim' : 'session-row'}>
      <div className="session-line">
        <FavoriteStar sessionId={session.id} />
        <SessionStateBadge state={session.displayState} />
        <span className="mono session-runtime">{formatDuration(sessionRuntime(session, now))}</span>
        <span className="session-activity">
          <Link className="session-title" to={`/sessions/${encodeURIComponent(session.id)}`}>
            {sessionTitle(session)}
          </Link>
          <span className="session-meta muted">
            <span className="mono" title={session.id}>
              {short}
            </span>
            <span>Last activity {formatAgo(session.lastEventAt, now)}</span>
          </span>
        </span>
        <span className="mono session-cwd" title={session.cwd}>
          {session.cwd}
        </span>
      </div>
      <Subagents session={session} now={now} />
    </li>
  );
}

function Group({ group, now }: { group: SessionGroup; now: number }) {
  const name = group.ticket ? `${group.ticket.key} ${group.ticket.title}` : 'Not assigned';
  return (
    <Panel as="section" className="panel session-group" aria-label={name}>
      <div className="session-group-head">
        {group.ticket ? (
          <>
            <span className="mono muted">{group.ticket.key}</span>
            <h2>{group.ticket.title}</h2>
          </>
        ) : (
          <>
            <h2>Not assigned</h2>
            <span className="muted">Sessions without a ticket.</span>
          </>
        )}
      </div>
      <ul className="session-list">
        {group.sessions.map((session) => (
          <SessionRow key={session.id} session={session} now={now} />
        ))}
      </ul>
    </Panel>
  );
}

function StateFilter({
  value,
  onChange,
}: {
  value: ReadonlySet<DisplayState>;
  onChange: (next: ReadonlySet<DisplayState>) => void;
}) {
  const all = value.size === 0;
  function toggle(state: DisplayState) {
    const next = new Set(value);
    if (!next.delete(state)) next.add(state);
    onChange(next);
  }
  return (
    <div className="session-filter" role="group" aria-label="Filter by state">
      <Button
        type="button"
        className="small"
        aria-pressed={all}
        onClick={() => {
          onChange(new Set());
        }}
      >
        All
      </Button>
      {STATES.map((state) => (
        <button
          key={state}
          type="button"
          className="btn small"
          aria-pressed={value.has(state)}
          onClick={() => {
            toggle(state);
          }}
        >
          {STATE_LABELS[state]}
        </button>
      ))}
    </div>
  );
}

function Sessions({ reported }: { reported: Session[] }) {
  const [filter, setFilter] = useState<ReadonlySet<DisplayState>>(DEFAULT_STATES);
  const { sessions, now } = useLiveSessions(reported);
  const groups = useMemo(
    () =>
      groupByTicket(
        sessions.filter((session) => filter.size === 0 || filter.has(session.displayState)),
      ),
    [sessions, filter],
  );
  const summary = summarize(sessions);
  return (
    <>
      {summary !== '' && <p className="muted session-summary">{summary}</p>}
      <FavoritesSection now={now} />
      <StateFilter value={filter} onChange={setFilter} />
      {sessions.length === 0 ? (
        <div className="state empty">
          <h2>No sessions</h2>
          <p>
            Sessions appear here once Claude Code reports them through the NULLRADIX plugin hooks.
          </p>
        </div>
      ) : (
        groups.length === 0 && <p className="muted">No sessions in these states.</p>
      )}
      {groups.map((group) => (
        <Group key={group.ticket?.key ?? 'loose'} group={group} now={now} />
      ))}
    </>
  );
}

// Claude Code sessions grouped by ticket; the change feed keeps the list current.
export function SessionsPage() {
  const { status, data } = useQuery(sessionsQuery());
  return (
    <div className="page">
      <h1 className="page-title">Sessions</h1>
      {status === 'pending' && <p className="muted">Loading sessions...</p>}
      {status === 'error' && <p className="form-error">Could not load the sessions.</p>}
      {status === 'success' && <Sessions reported={data} />}
    </div>
  );
}
