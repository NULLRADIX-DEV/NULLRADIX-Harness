import { useId, useState } from 'react';
import { useNow } from '../../shared/lib/useNow';
import { isActive, type Run } from './queries';
import { RunBody } from './RunBody';
import { RunHead } from './RunHead';

// One run as a disclosure: the head always, the body once opened.
export function RunView({ run, defaultOpen = false }: { run: Run; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();
  const now = useNow(isActive(run.status));
  return (
    <section className="run-card" aria-label={`Run ${run.label}`}>
      <button
        type="button"
        className="run-card-head"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => {
          setOpen((current) => !current);
        }}
      >
        <span aria-hidden="true">{open ? '\u25be' : '\u25b8'}</span>
        <RunHead run={run} now={now} />
      </button>
      <div id={bodyId} hidden={!open}>
        {open && <RunBody run={run} />}
      </div>
    </section>
  );
}
