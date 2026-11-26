import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { useSearchParams } from 'react-router';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import { boardsQuery } from '../boards/queries';
import { dispatcherConfigQuery } from '../dispatcher/queries';
import { DispatcherIndicator } from '../dispatcher/DispatcherIndicator';
import { RunsSection } from '../runs/RunsSection';
import { formatDay, isoDay } from './format';
import { boardStatusQuery, statsQuery, type DispatchStats } from './queries';
import {
  KINDS,
  PERIODS,
  filterParams,
  parseFilter,
  startOf,
  statsParams,
  withEdge,
  DEFAULT_PERIOD,
  type DashboardFilter,
  type Period,
  type StatsKind,
} from './range';
import { RunsChart } from './RunsChart';
import { AbortReasonsCard, KpiGrid, LadderCard, ObstaclesCard, UsageCard } from './StatsCards';
import { TodoColumn } from './TodoColumn';
import './dashboard.css';

function Segmented<T extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: readonly { value: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
}) {
  return (
    <div className="segmented" role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          className={option.value === value ? 'on' : undefined}
          aria-pressed={option.value === value}
          onClick={() => {
            onChange(option.value);
          }}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

function Content({
  stats,
  boardId,
  ladder,
  now,
}: {
  stats: DispatchStats;
  boardId: string | null;
  ladder: readonly string[];
  now: number;
}) {
  const status = useQuery(boardStatusQuery(boardId));
  const noun = stats.series.unit === 'day' ? 'day' : 'week';
  return (
    <>
      {stats.kpis.runs === 0 && (
        <p className="dashboard-note" role="note">
          {`No ${stats.filter.kind} runs between ${formatDay(stats.filter.from, true)} and ${formatDay(stats.filter.to, true)}.`}
        </p>
      )}
      <KpiGrid stats={stats} />
      <div className="dash-main">
        <div className="dash-left">
          <section className="dash-card" aria-labelledby="dash-series">
            <div className="dash-card-head">
              <h2 id="dash-series">{`Runs per ${noun}`}</h2>
              <div className="chart-legend">
                <span>
                  <i className="dot done" aria-hidden="true" />
                  done
                </span>
                <span>
                  <i className="dot fail" aria-hidden="true" />
                  aborted
                </span>
                <span>
                  <i className="dot other" aria-hidden="true" />
                  question / blocked
                </span>
              </div>
            </div>
            <RunsChart series={stats.series} />
          </section>
          <div className="dash-pair">
            <LadderCard stats={stats} ladder={stats.filter.kind === 'work' ? ladder : []} />
            <AbortReasonsCard stats={stats} />
          </div>
          <ObstaclesCard status={status.data} stats={stats} />
          <UsageCard stats={stats} />
        </div>
        <TodoColumn boardId={boardId} now={now} />
      </div>
    </>
  );
}

// Dispatcher numbers over a range, and what waits for the person; filters live in the address.
export function DashboardPage({ today }: { today?: Date }) {
  const day = today ?? new Date();
  const [search, setSearch] = useSearchParams();
  const filter = parseFilter(search);
  const params = statsParams(filter, day);
  const boards = useQuery(boardsQuery());
  const config = useQuery(dispatcherConfigQuery());
  // the last answer stays while the next range loads, so "since config change" keeps its date
  const stats = useQuery({ ...statsQuery(params), placeholderData: keepPreviousData });
  const loaded = stats.data;
  const changedAt = loaded?.configChangedAt ?? null;

  function change(next: Partial<DashboardFilter>) {
    setSearch(filterParams({ ...filter, ...next }), { replace: true });
  }

  function choosePeriod(period: Period) {
    if (period === 'custom') {
      change({
        period,
        from: loaded?.filter.from ?? startOf(day, Number(DEFAULT_PERIOD)),
        to: loaded?.filter.to ?? isoDay(day),
      });
    } else {
      change({ period });
    }
  }

  function setEdge(edge: 'from' | 'to', value: string) {
    if (filter.from === undefined || filter.to === undefined) return;
    change(withEdge({ from: filter.from, to: filter.to }, edge, value));
  }

  const error = stats.error
    ? problemMessage(stats.error, {}, 'Could not load the statistics.')
    : null;

  return (
    <div className="page dashboard">
      <header className="dash-head">
        <div className="dash-title">
          <h1>Dashboard</h1>
        </div>
        <label className="field">
          <span className="visually-hidden">Board</span>
          <select
            aria-label="Board"
            value={filter.boardId ?? ''}
            onChange={(event) => {
              change({ boardId: event.target.value || null });
            }}
          >
            <option value="">All boards</option>
            {boards.data?.map((board) => (
              <option key={board.id} value={board.id}>
                {`${board.projectKey} / ${board.name}`}
              </option>
            ))}
          </select>
        </label>
        <Segmented<StatsKind>
          label="Run kind"
          options={KINDS}
          value={filter.kind}
          onChange={(kind) => {
            change({ kind });
          }}
        />
        <select
          aria-label="Range"
          value={filter.period}
          onChange={(event) => {
            choosePeriod(event.target.value as Period);
          }}
        >
          {PERIODS.map((period) =>
            period.value === 'config' ? (
              <option
                key={period.value}
                value={period.value}
                disabled={changedAt === null && filter.period !== 'config'}
              >
                {changedAt === null
                  ? `${period.label} (none yet)`
                  : `${period.label} (${formatDay(isoDay(new Date(changedAt)), true)})`}
              </option>
            ) : (
              <option key={period.value} value={period.value}>
                {period.label}
              </option>
            ),
          )}
        </select>
        {filter.period === 'custom' && filter.from !== undefined && filter.to !== undefined && (
          <div className="dash-range" role="group" aria-label="Custom range">
            <input
              type="date"
              aria-label="From"
              value={filter.from}
              max={isoDay(day)}
              onChange={(event) => {
                setEdge('from', event.target.value);
              }}
            />
            <span aria-hidden="true">-</span>
            <input
              type="date"
              aria-label="To"
              value={filter.to}
              max={isoDay(day)}
              onChange={(event) => {
                setEdge('to', event.target.value);
              }}
            />
          </div>
        )}
        <DispatcherIndicator />
      </header>
      {stats.status === 'pending' && <p className="muted">Loading the statistics...</p>}
      {error !== null && <FormError message={error} />}
      {loaded && (
        <div
          aria-busy={stats.isPlaceholderData}
          className={`dash-body${stats.isPlaceholderData ? ' dash-stale' : ''}`}
        >
          <Content
            stats={loaded}
            boardId={filter.boardId}
            ladder={config.data?.config.ladder ?? []}
            now={day.getTime()}
          />
        </div>
      )}
      <RunsSection />
    </div>
  );
}
