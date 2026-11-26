import type { DispatcherStatus } from '../dispatcher/statusQueries';
import { NONE, formatCost, formatCount, formatDuration, formatPercent, plural } from './format';
import type { DispatchStats, ProfileRow } from './queries';

export type KpiTone = 'neutral' | 'good' | 'warn' | 'bad';

// From this escalation rate on the value turns orange.
export const ESCALATION_WARN = 0.3;

// A figure with its label; the explanation is the tooltip.
export function KpiTile({
  label,
  value,
  sub,
  tone = 'neutral',
}: {
  label: string;
  value: string;
  sub?: string;
  tone?: KpiTone;
}) {
  return (
    <section className="kpi" aria-label={label} title={sub}>
      <h3 className="kpi-label">{label}</h3>
      <p className={`kpi-value mono tone-${tone}`}>{value}</p>
      {sub && <span className="visually-hidden">{sub}</span>}
    </section>
  );
}

export function KpiGrid({ stats }: { stats: DispatchStats }) {
  const { kpis } = stats;
  const subjects = stats.filter.kind === 'research' ? 'questions' : 'tickets';
  return (
    <div className="kpi-grid">
      <KpiTile
        label="Runs"
        value={String(kpis.runs)}
        sub={`${plural(kpis.subjects, subjects.slice(0, -1), subjects)} in the range`}
      />
      <KpiTile
        label="Success rate"
        value={formatPercent(kpis.successRate)}
        sub={`${String(kpis.solved)} solved, ${String(kpis.finalFailures)} failed for good`}
      />
      <KpiTile
        label="First-try rate"
        value={formatPercent(kpis.firstTryRate)}
        sub="Solved by the first run, without an escalation"
        tone="good"
      />
      <KpiTile
        label="Escalation rate"
        value={formatPercent(kpis.escalationRate)}
        sub={`Share of ${subjects} with at least one escalation`}
        tone={(kpis.escalationRate ?? 0) > ESCALATION_WARN ? 'warn' : 'neutral'}
      />
      <KpiTile
        label="Failed for good"
        value={String(kpis.finalFailures)}
        sub="Attempts used up"
        tone={kpis.finalFailures > 0 ? 'bad' : 'neutral'}
      />
    </div>
  );
}

// Profiles in the order of the ladder, then others with runs by their share.
export function ladderRows(rows: readonly ProfileRow[], ladder: readonly string[]): ProfileRow[] {
  const empty = (profile: string): ProfileRow => ({
    profile,
    runs: 0,
    done: 0,
    escalated: 0,
    final: 0,
    other: 0,
  });
  const onLadder = ladder.map((name) => rows.find((row) => row.profile === name) ?? empty(name));
  return [...onLadder, ...rows.filter((row) => !ladder.includes(row.profile))];
}

function ladderTitle(row: ProfileRow): string {
  if (row.runs === 0) return 'no runs';
  const part = (n: number, text: string) =>
    `${formatPercent(n / row.runs)} ${text} (${plural(n, 'run', 'runs')})`;
  const parts = [part(row.done, 'solved')];
  if (row.escalated > 0 || row.final === 0) parts.push(part(row.escalated, 'escalated'));
  if (row.final > 0) parts.push(part(row.final, 'failed for good'));
  if (row.other > 0) parts.push(part(row.other, 'question / blocked'));
  return parts.join(' / ');
}

// One bar per profile: solved, escalated, failed for good; below the end of the finished subjects.
export function LadderCard({ stats, ladder }: { stats: DispatchStats; ladder: readonly string[] }) {
  const rows = ladderRows(stats.byProfile, ladder);
  const summary = stats.ladder;
  return (
    <section className="dash-card" aria-labelledby="dash-ladder">
      <h2 id="dash-ladder">Escalation along the ladder</h2>
      {rows.length === 0 ? (
        <p className="muted dash-empty">No runs in the range.</p>
      ) : (
        <>
          <ul className="bar-list">
            {rows.map((row) => {
              const segments = [
                { key: 'done', n: row.done, text: 'solved' },
                { key: 'escalated', n: row.escalated, text: 'escalated' },
                { key: 'final', n: row.final, text: 'failed for good' },
              ].filter((segment) => segment.n > 0);
              return (
                <li key={row.profile} title={ladderTitle(row)}>
                  <div className="bar-head">
                    <span className="mono ladder-name">{row.profile}</span>
                    <span className="bar-runs muted">{plural(row.runs, 'run', 'runs')}</span>
                    <span className="bar-value mono tone-good">
                      {row.runs > 0 ? formatPercent(row.done / row.runs) : NONE}
                    </span>
                  </div>
                  <div className="ladder-bar" aria-hidden="true">
                    {segments.map((segment) => (
                      <div
                        key={segment.key}
                        className={segment.key}
                        style={{ width: `${String((segment.n / row.runs) * 100)}%` }}
                      />
                    ))}
                  </div>
                  <span className="visually-hidden">{ladderTitle(row)}</span>
                </li>
              );
            })}
          </ul>
          {summary.finished > 0 && (
            <dl
              className="ladder-summary"
              title={`Outcome of ${String(summary.finished)} finished tickets or questions`}
            >
              <div>
                <dt className="mono tone-good">{formatPercent(summary.firstTry)}</dt>
                <dd>solved on the first try</dd>
              </div>
              <div>
                <dt className="mono tone-escalated">{formatPercent(summary.afterEscalation)}</dt>
                <dd>solved after an escalation</dd>
              </div>
              <div>
                <dt className="mono tone-bad">{formatPercent(summary.final)}</dt>
                <dd>failed for good</dd>
              </div>
            </dl>
          )}
        </>
      )}
    </section>
  );
}

// This many reasons the card shows.
const TOP_REASONS = 5;
// Categories with a colour of their own; the rest is grey.
const COLORED = new Set([
  'max-turns',
  'timeout',
  'context',
  'exit-code',
  'claude-error',
  'no-outcome',
]);

// The most frequent abort categories with their share of all aborts.
export function AbortReasonsCard({ stats }: { stats: DispatchStats }) {
  const reasons = stats.abortReasons.slice(0, TOP_REASONS);
  const total = stats.abortReasons.reduce((sum, reason) => sum + reason.count, 0);
  const max = Math.max(1, ...reasons.map((reason) => reason.count));
  const configurable =
    stats.configurableAbortShare === null || stats.configurableAbortShare === undefined
      ? undefined
      : `${formatPercent(stats.configurableAbortShare)} of the aborts come from limits or a full context and can be tuned in the config.`;
  return (
    <section className="dash-card" aria-labelledby="dash-reasons">
      <h2 id="dash-reasons" title={configurable}>
        Most frequent abort reasons
      </h2>
      {reasons.length === 0 ? (
        <p className="muted dash-empty">No aborts in the range.</p>
      ) : (
        <ul className="bar-list">
          {reasons.map((reason) => {
            const category = `cat-${COLORED.has(reason.category) ? reason.category : 'rest'}`;
            const share = formatPercent(reason.count / total);
            return (
              <li
                key={reason.category}
                title={`${reason.label}: ${plural(reason.count, 'abort', 'aborts')} of ${String(total)} (${share})`}
              >
                <div className="bar-head">
                  <span className="reason-name">{reason.label}</span>
                  <span className="bar-runs muted">{plural(reason.count, 'run', 'runs')}</span>
                  <span className={`bar-value mono reason-text ${category}`}>{share}</span>
                </div>
                <div className="reason-track" aria-hidden="true">
                  <div
                    className={`reason-fill ${category}`}
                    style={{ width: `${String((reason.count / max) * 100)}%` }}
                  />
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {configurable && <p className="dash-foot muted">{configurable}</p>}
    </section>
  );
}

// Readiness reasons that wait for another ticket, a merge or a question.
const WAITING = /^waits for (?:the merge of |question )?/;
// Reasons that hang on a label (opt-in missing or opt-out set).
const LABEL_PREFIX = 'label "';
const KEY = /[A-Z][A-Z0-9]*-[A-Z]?\d+/g;
// This many keys the dependency line names.
const KEYS_SHOWN = 3;

interface Obstacle {
  count: number | null;
  label: string;
  sub: string;
}

// What slows the dispatcher: readiness from the live state, pauses and lost runs from stats.
export function obstacles(
  status: DispatcherStatus | undefined,
  stats: DispatchStats | undefined,
): Obstacle[] {
  const tickets = status?.tickets ?? null;
  const waiting = tickets?.filter((ticket) => WAITING.test(ticket.reason)) ?? [];
  const blockers = [...new Set(waiting.flatMap((ticket) => ticket.reason.match(KEY) ?? []))];
  const labelled = tickets?.filter((ticket) => ticket.reason.startsWith(LABEL_PREFIX)) ?? [];
  const labels = [...new Set(labelled.map((ticket) => ticket.reason))];
  return [
    {
      count: tickets ? waiting.length : null,
      label: 'Blocked by a dependency',
      sub:
        blockers.length > 0
          ? `planned, waiting for ${blockers.slice(0, KEYS_SHOWN).join(', ')}${blockers.length > KEYS_SHOWN ? ' ...' : ''}`
          : 'planned, waiting for other tickets',
    },
    {
      count: tickets ? labelled.length : null,
      label: 'Left out by a label',
      sub: labels.length > 0 ? labels.join(' / ') : 'Opt-in missing or opt-out set',
    },
    { count: stats?.usagePauses ?? null, label: 'Usage limit pauses', sub: 'in the range' },
    {
      count: stats?.staleRuns ?? null,
      label: 'Stale runs',
      sub: 'lost by the worker, in the range',
    },
  ];
}

export function ObstaclesCard({
  status,
  stats,
}: {
  status: DispatcherStatus | undefined;
  stats: DispatchStats | undefined;
}) {
  return (
    <section className="dash-card obstacles" aria-labelledby="dash-obstacles">
      <h2 id="dash-obstacles">Obstacles</h2>
      <ul>
        {obstacles(status, stats).map((obstacle) => (
          <li key={obstacle.label}>
            <span className="mono obstacle-count">{obstacle.count ?? NONE}</span>
            <span className="obstacle-text">
              <strong>{obstacle.label}</strong>
              <span className="muted">{obstacle.sub}</span>
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

export const OUTCOME_LABELS: Readonly<Record<string, string>> = {
  done: 'Done',
  review: 'Held for review',
  human: 'Needs a person',
  abort: 'Aborted',
  'abort-final': 'Failed for good',
  blocked: 'Blocked',
  question: 'Question',
  'pause-limit': 'Paused (usage limit)',
  'pause-claude-missing': 'Paused (Claude missing)',
  'setup-failed': 'Setup failed',
  taken: 'Taken over',
  gone: 'Gone',
  other: 'Other',
  'merge-session': 'Merge session',
  research: 'Researched',
  planned: 'Planned',
};

// Cost, tokens and time of the runs counted, and how every run ended.
export function UsageCard({ stats }: { stats: DispatchStats }) {
  const { usage } = stats;
  const figures: [string, string, string?][] = [
    ['Cost', formatCost(usage.costUsd)],
    ['Average duration', formatDuration(usage.averageDurationMs)],
    ['Input tokens', formatCount(usage.inputTokens), String(usage.inputTokens)],
    ['Output tokens', formatCount(usage.outputTokens), String(usage.outputTokens)],
    ['Cache reads', formatCount(usage.cacheReadTokens), String(usage.cacheReadTokens)],
    ['Cache writes', formatCount(usage.cacheWriteTokens), String(usage.cacheWriteTokens)],
  ];
  return (
    <section className="dash-card" aria-labelledby="dash-usage">
      <h2 id="dash-usage">Usage and outcomes</h2>
      <dl className="usage-grid">
        {figures.map(([label, value, exact]) => (
          <div key={label} title={exact}>
            <dt>{label}</dt>
            <dd className="mono">{value}</dd>
          </div>
        ))}
      </dl>
      {stats.outcomes.length === 0 ? (
        <p className="muted dash-empty">No runs in the range.</p>
      ) : (
        <ul className="outcome-list">
          {stats.outcomes.map((row) => (
            <li key={row.outcome}>
              <span>{OUTCOME_LABELS[row.outcome] ?? row.outcome}</span>
              <span className="mono">{row.count}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
