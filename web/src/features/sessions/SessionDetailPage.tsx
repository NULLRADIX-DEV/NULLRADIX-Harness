import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState, type ReactNode } from 'react';
import { Link, useParams } from 'react-router';
import { ChangesView } from './ChangesView';
import { formatDuration, sessionRuntime, sessionTitle, withLiveState } from './format';
import { sessionDetailQuery, sessionKeys, type SessionClient, type SessionDetail } from './queries';
import { SessionStateBadge } from './SessionStateBadge';
import { TranscriptView } from './TranscriptView';
import { useTranscript } from './transcript';
import { useNow } from '../../shared/lib/useNow';

type Tab = 'transcript' | 'changes';

export const CLIENT_LABELS: Readonly<Record<SessionClient, string>> = {
  nullradix: 'NULLRADIX',
  'claude-code': 'Claude Code',
  'claude-desktop': 'Desktop',
  unknown: 'Unknown client',
};

const TODO_MARKS = { pending: ' ', 'in-progress': '>', completed: 'x' } as const;

export function formatCost(usd: number): string {
  const digits = usd !== 0 && Math.abs(usd) < 0.01 ? 3 : 2;
  return `$${usd.toFixed(digits)}`;
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="sd-fact">
      <dt className="muted">{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

function Facts({ detail, now }: { detail: SessionDetail; now: number }) {
  const session = withLiveState(detail.session, now);
  const { tokens } = detail;
  return (
    <dl className="sd-facts">
      {session.model && (
        <Fact label="Model">
          <span className="mono">{session.model}</span>
        </Fact>
      )}
      <Fact label="Runtime">{formatDuration(sessionRuntime(session, now))}</Fact>
      {detail.branch && (
        <Fact label="Branch">
          <span className="mono">{detail.branch}</span>
        </Fact>
      )}
      {session.tickets.length > 0 && (
        <Fact label="Tickets">
          {session.tickets.map((ticket, index) => (
            <span key={ticket.key}>
              {index > 0 && ', '}
              <Link to={`/board?ticket=${ticket.key}`} title={ticket.title} className="mono">
                {ticket.key}
              </Link>
            </span>
          ))}
        </Fact>
      )}
      {detail.costUsd != null && <Fact label="Cost">{formatCost(detail.costUsd)}</Fact>}
      <Fact label="Tokens">
        {`${String(tokens.input)} in, ${String(tokens.output)} out, ${String(tokens.cacheRead)} cache read`}
      </Fact>
      {detail.context && (
        <Fact label="Context">
          {detail.context.limitTokens == null
            ? String(detail.context.usedTokens)
            : `${String(detail.context.usedTokens)} of ${String(detail.context.limitTokens)}`}
        </Fact>
      )}
      <Fact label="Folder">
        <span className="mono">{session.cwd}</span>
      </Fact>
      {detail.claudeVersion && <Fact label="Claude Code">{detail.claudeVersion}</Fact>}
    </dl>
  );
}

function Todos({ detail }: { detail: SessionDetail }) {
  if (detail.todos.length === 0) return null;
  return (
    <section className="panel sd-todos" aria-label="Todos">
      <h2>Todos</h2>
      <ul>
        {detail.todos.map((todo, index) => (
          <li key={index} className={`sd-todo ${todo.status}`}>
            <span className="mono" aria-hidden="true">
              [{TODO_MARKS[todo.status]}]
            </span>{' '}
            {todo.content}
          </li>
        ))}
      </ul>
    </section>
  );
}

function Detail({
  sessionId,
  detail,
  version,
}: {
  sessionId: string;
  detail: SessionDetail;
  version: number;
}) {
  const [tab, setTab] = useState<Tab>('transcript');
  const now = useNow(true);
  const { transcript, retry } = useTranscript(sessionId, version);
  const session = withLiveState(detail.session, now);
  return (
    <>
      <div className="sd-kicker">
        <span className="badge">{CLIENT_LABELS[detail.client]}</span>
        <SessionStateBadge state={session.displayState} />
        <span className="mono muted" title={session.id}>
          {session.id.slice(0, 8)}
        </span>
      </div>
      <h1 className="page-title">{sessionTitle(session)}</h1>
      <Facts detail={detail} now={now} />
      <Todos detail={detail} />
      <div role="tablist" aria-label="Session detail" className="sd-tabs">
        {(['transcript', 'changes'] as const).map((name) => (
          <button
            key={name}
            type="button"
            role="tab"
            aria-selected={tab === name}
            className={tab === name ? 'btn small on' : 'btn small'}
            onClick={() => {
              setTab(name);
            }}
          >
            {name === 'transcript' ? 'Transcript' : 'Changes'}
          </button>
        ))}
      </div>
      <div role="tabpanel" className="sd-panel">
        {tab === 'transcript' ? (
          <TranscriptView
            transcript={transcript}
            running={session.displayState === 'running'}
            onRetry={retry}
          />
        ) : (
          <ChangesView sessionId={sessionId} />
        )}
      </div>
    </>
  );
}

// One session with its figures, transcript and changes; refreshed while it runs.
export function SessionDetailPage() {
  const { sessionId = '' } = useParams();
  const queryClient = useQueryClient();
  const { status, data, dataUpdatedAt } = useQuery(sessionDetailQuery(sessionId));
  // a newer detail means the transcript grew, so changes and open diffs read again
  useEffect(() => {
    if (dataUpdatedAt === 0) return;
    void queryClient.invalidateQueries({ queryKey: sessionKeys.changes(sessionId) });
    void queryClient.invalidateQueries({ queryKey: [...sessionKeys.all, 'diff', sessionId] });
  }, [queryClient, sessionId, dataUpdatedAt]);
  return (
    <div className="page session-detail">
      <p>
        <Link to="/sessions">All sessions</Link>
      </p>
      {status === 'pending' && <p className="muted">Loading the session...</p>}
      {status === 'error' && <p className="form-error">Could not load the session.</p>}
      {status === 'success' && (
        <Detail key={sessionId} sessionId={sessionId} detail={data} version={dataUpdatedAt} />
      )}
    </div>
  );
}
