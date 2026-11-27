import { useQuery } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { useNow } from '../../shared/lib/useNow';
import { FormError } from '../../shared/ui/FormError';
import { STATUS_LABELS, formatStart, runDuration } from './format';
import { isActive, runsQuery, useCancelRun, useRunOutput, type Run } from './queries';
import { Button } from '../../shared/ui/Button';
import { Panel } from '../../shared/ui/Panel';

export const FINISHED_MESSAGE = 'This run has already finished.';
export const GONE_MESSAGE = 'This run no longer exists.';

const MESSAGES = {
  'run-finished': FINISHED_MESSAGE,
  'run-not-found': GONE_MESSAGE,
};

function RunStatusBadge({ run }: { run: Run }) {
  return <span className={`run-status run-status-${run.status}`}>{STATUS_LABELS[run.status]}</span>;
}

function RunRow({
  run,
  now,
  selected,
  onSelect,
  buttonRef,
}: {
  run: Run;
  now: number;
  selected: boolean;
  onSelect: () => void;
  buttonRef: (button: HTMLButtonElement | null) => void;
}) {
  const duration = runDuration(run, now);
  return (
    <li>
      <button
        ref={buttonRef}
        type="button"
        className="run-row"
        aria-pressed={selected}
        onClick={onSelect}
      >
        <RunStatusBadge run={run} />
        <span className="run-label">{run.label}</span>
        <span className="run-meta muted">
          <span>{formatStart(run)}</span>
          {duration !== null && <span>{duration}</span>}
          {run.exitCode !== null && run.exitCode !== undefined && (
            <span>exit {String(run.exitCode)}</span>
          )}
        </span>
      </button>
    </li>
  );
}

// Keeps the view at the newest line while the reader has not scrolled up.
function useStickToBottom(newest: number) {
  const ref = useRef<HTMLPreElement>(null);
  const atBottom = useRef(true);

  useEffect(() => {
    const element = ref.current;
    if (element && atBottom.current) element.scrollTop = element.scrollHeight;
  }, [newest]);

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
  const { ref, onScroll } = useStickToBottom(output.lines.at(-1)?.seq ?? 0);
  // the output may report the end before the list does
  const active = isActive(run.status) && !output.finished;

  return (
    <div className="run-output" role="region" aria-labelledby={headingId}>
      <div className="run-output-head">
        <h3 id={headingId}>{run.label}</h3>
        <RunStatusBadge run={run} />
        <span className="run-output-actions">
          {active && (
            <Button
              type="button"
              variant="danger"
              disabled={cancel.isPending}
              onClick={() => {
                cancel.mutate(run.id);
              }}
            >
              Cancel run
            </Button>
          )}
          <Button type="button" onClick={onClose}>
            Close
          </Button>
        </span>
      </div>
      {active && (
        <FormError message={problemMessage(cancel.error, MESSAGES, 'Could not cancel the run.')} />
      )}
      {output.truncated && <p className="muted">Earlier lines are not shown.</p>}
      <pre
        ref={ref}
        className="run-lines"
        role="log"
        aria-live="off"
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
      {output.lines.length === 0 && !output.refused && (
        <p className="muted">{output.finished ? 'The run printed nothing.' : 'No output yet.'}</p>
      )}
      <FormError message={problemMessage(output.refused, MESSAGES, 'Could not load the output.')} />
      {output.retrying && <p className="form-error">Cannot reach the output, retrying.</p>}
      {run.error && <p className="form-error">{run.error}</p>}
    </div>
  );
}

function RunList({
  runs,
  selectedId,
  onSelect,
  buttonRef,
}: {
  runs: Run[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  buttonRef: (id: string) => (button: HTMLButtonElement | null) => void;
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
          buttonRef={buttonRef(run.id)}
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
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const { status, data } = useQuery(runsQuery());
  const selected = data?.find((run) => run.id === selectedId);

  const buttonRef = (id: string) => (button: HTMLButtonElement | null) => {
    if (button) rows.current.set(id, button);
    else rows.current.delete(id);
  };

  function close(id: string) {
    setSelectedId(null);
    // hand focus back to the row that opened the output
    rows.current.get(id)?.focus();
  }

  return (
    <Panel as="section" className="panel page-section" aria-labelledby={headingId}>
      <h2 id={headingId}>Runs</h2>
      {status === 'pending' && <p className="muted">Loading runs...</p>}
      {status === 'error' && <p className="form-error">Could not load the runs.</p>}
      {status === 'success' && (
        <RunList
          runs={data}
          selectedId={selectedId}
          buttonRef={buttonRef}
          onSelect={(id) => {
            setSelectedId((current) => (current === id ? null : id));
          }}
        />
      )}
      {selected && (
        <RunOutput
          key={selected.id}
          run={selected}
          onClose={() => {
            close(selected.id);
          }}
        />
      )}
    </Panel>
  );
}
