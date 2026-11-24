import { useQuery } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { useNow } from '../../shared/lib/useNow';
import { FormError } from '../../shared/ui/FormError';
import { STATUS_LABELS, formatStart, runDuration } from './format';
import { isActive, runsQuery, useCancelRun, useRunOutput, type Run } from './queries';

export const FINISHED_MESSAGE = 'This run has already finished.';

const MESSAGES = {
  'run-finished': FINISHED_MESSAGE,
  'run-not-found': 'This run no longer exists.',
};

function RunStatusBadge({ run }: { run: Run }) {
  return <span className={`run-status run-status-${run.status}`}>{STATUS_LABELS[run.status]}</span>;
}

function RunRow({
  run,
  now,
  selected,
  onSelect,
}: {
  run: Run;
  now: number;
  selected: boolean;
  onSelect: () => void;
}) {
  const duration = runDuration(run, now);
  return (
    <li>
      <button type="button" className="run-row" aria-pressed={selected} onClick={onSelect}>
        <RunStatusBadge run={run} />
        <span className="run-label">{run.label}</span>
        <span className="run-meta muted">
          {formatStart(run)}
          {duration !== null && ` · ${duration}`}
          {run.exitCode !== null && run.exitCode !== undefined && ` · exit ${String(run.exitCode)}`}
        </span>
      </button>
    </li>
  );
}

// Keeps the view at the newest line while the reader has not scrolled up.
function useStickToBottom(length: number) {
  const ref = useRef<HTMLPreElement>(null);
  const atBottom = useRef(true);

  useEffect(() => {
    const element = ref.current;
    if (element && atBottom.current) element.scrollTop = element.scrollHeight;
  }, [length]);

  function onScroll() {
    const element = ref.current;
    if (!element) return;
    atBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
  }

  return { ref, onScroll };
}

function RunOutput({ run, onClose }: { run: Run; onClose: () => void }) {
  const output = useRunOutput(run.id);
  const cancel = useCancelRun();
  const headingId = useId();
  const { ref, onScroll } = useStickToBottom(output.lines.length);
  const active = isActive(run.status);

  return (
    <div className="run-output" role="region" aria-labelledby={headingId}>
      <div className="run-output-head">
        <h3 id={headingId}>{run.label}</h3>
        <RunStatusBadge run={run} />
        <span className="run-output-actions">
          {active && (
            <button
              type="button"
              className="btn danger"
              disabled={cancel.isPending}
              onClick={() => {
                cancel.mutate(run.id);
              }}
            >
              Cancel run
            </button>
          )}
          <button type="button" className="btn" onClick={onClose}>
            Close
          </button>
        </span>
      </div>
      <FormError message={problemMessage(cancel.error, MESSAGES, 'Could not cancel the run.')} />
      {output.truncated && <p className="muted">Earlier lines are not shown.</p>}
      <pre
        ref={ref}
        className="run-lines"
        tabIndex={0}
        aria-label={`Output of ${run.label}`}
        onScroll={onScroll}
      >
        {output.lines.map((line) => (
          <span
            key={line.seq}
            className={line.stream === 'stderr' ? 'run-line stderr' : 'run-line'}
          >
            {line.text}
            {'\n'}
          </span>
        ))}
      </pre>
      {output.lines.length === 0 && (
        <p className="muted">{output.finished ? 'The run printed nothing.' : 'No output yet.'}</p>
      )}
      {output.failing && <p className="form-error">Cannot reach the output, retrying.</p>}
      {run.error && <p className="form-error">{run.error}</p>}
    </div>
  );
}

function RunList({
  runs,
  selectedId,
  onSelect,
}: {
  runs: Run[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const now = useNow(runs.some((run) => isActive(run.status)));
  if (runs.length === 0) {
    return (
      <p className="muted">
        No runs yet. Start one with <code>nullradix run start -- PROGRAM</code>.
      </p>
    );
  }
  return (
    <ul className="run-list" aria-label="Runs">
      {runs.map((run) => (
        <RunRow
          key={run.id}
          run={run}
          now={now}
          selected={run.id === selectedId}
          onSelect={() => {
            onSelect(run.id);
          }}
        />
      ))}
    </ul>
  );
}

export function RunsSection() {
  const headingId = useId();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const { status, data } = useQuery(runsQuery());
  const selected = data?.find((run) => run.id === selectedId);

  return (
    <section className="panel page-section" aria-labelledby={headingId}>
      <h2 id={headingId}>Runs</h2>
      {status === 'pending' && <p className="muted">Loading runs...</p>}
      {status === 'error' && <p className="form-error">Could not load the runs.</p>}
      {status === 'success' && (
        <RunList runs={data} selectedId={selectedId} onSelect={setSelectedId} />
      )}
      {selected && (
        <RunOutput
          key={selected.id}
          run={selected}
          onClose={() => {
            setSelectedId(null);
          }}
        />
      )}
    </section>
  );
}
