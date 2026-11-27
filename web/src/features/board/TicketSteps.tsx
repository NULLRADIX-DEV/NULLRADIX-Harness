import { useEffect, useRef, useState } from 'react';
import { Progress } from '../../shared/ui/Progress';
import type { Step } from '../tickets/queries';
import { StepForm } from './StepForm';
import type { TicketActions } from './useTicketActions';
import { Button } from '../../shared/ui/Button';

// `text` is what the step said when the form opened; the daemon refuses a step changed since.
type Mode = { kind: 'add' } | { kind: 'edit' | 'remove'; id: string; text: string } | null;

// Number of the step an open form belongs to after a change of the list; 0 when it is gone.
function locate(steps: readonly Step[], id: string): number {
  return steps.findIndex((step) => step.id === id) + 1;
}

function guidance(detail: string): string | null {
  return detail.trim() === '' ? null : detail;
}

function StepsProgress({ done, total }: { done: number; total: number }) {
  return (
    <Progress done={done} total={total} label="Steps done" caption="steps done">
      <div className="progress">
        <span
          className="progress-bar"
          role="progressbar"
          aria-label="Steps done"
          aria-valuemin={0}
          aria-valuemax={total}
          aria-valuenow={done}
        >
          <span style={{ width: `${String(Math.round((done / total) * 100))}%` }} />
        </span>
        <span className="progress-text">
          {done} of {total} done
        </span>
      </div>
    </Progress>
  );
}

// Every change names the step text it saw, so the daemon refuses it on a changed list.
export function TicketSteps({
  steps,
  actions,
}: {
  steps: readonly Step[];
  actions: TicketActions;
}) {
  const [mode, setMode] = useState<Mode>(null);
  const addButton = useRef<HTMLButtonElement>(null);
  const wasOpen = useRef(false);
  const target = mode && mode.kind !== 'add' ? locate(steps, mode.id) : 0;
  const open = mode?.kind === 'add' || target > 0;
  const locked = actions.busy || open;
  const done = steps.filter((step) => step.done).length;

  useEffect(() => {
    // hand focus back once a form or confirmation closes
    if (!open && wasOpen.current) addButton.current?.focus();
    wasOpen.current = open;
  }, [open]);

  function close() {
    setMode(null);
  }

  return (
    <section className="db-steps" aria-label="Steps">
      {steps.length > 0 ? (
        <StepsProgress done={done} total={steps.length} />
      ) : (
        <p className="muted">No steps yet.</p>
      )}
      {steps.length > 0 && (
        <ol className="db-step-list">
          {steps.map((step, index) => {
            const number = index + 1;
            const name = `step ${String(number)}`;
            if (mode?.kind === 'edit' && target === number) {
              return (
                <li key="edit-form" className="db-step">
                  <StepForm
                    label={`Edit ${name}`}
                    initial={{ text: step.text, detail: step.detail ?? '' }}
                    busy={actions.busy}
                    submitLabel="Save"
                    onCancel={close}
                    onSave={(value) => {
                      void actions
                        .changeStep(step.id, {
                          text: value.text,
                          detail: guidance(value.detail),
                          expectText: mode.text,
                        })
                        .then((saved) => {
                          if (saved) close();
                        });
                    }}
                  />
                </li>
              );
            }
            const confirming = mode?.kind === 'remove' && target === number;
            return (
              <li key={step.id} className={step.done ? 'db-step done' : 'db-step'}>
                <div className="db-step-line">
                  <label className="db-step-row">
                    <input
                      type="checkbox"
                      checked={step.done}
                      aria-disabled={actions.busy}
                      aria-label={`Step ${String(number)}: ${step.text}`}
                      onChange={(event) => {
                        if (actions.busy) return;
                        void actions.changeStep(step.id, {
                          done: event.target.checked,
                          expectText: step.text,
                        });
                      }}
                    />
                    <span className="db-step-text">{step.text}</span>
                  </label>
                  <span className="db-step-tools">
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={`Move ${name} up`}
                      title="Move up"
                      disabled={number === 1}
                      aria-disabled={locked}
                      onClick={() => {
                        if (!locked) void actions.moveStep(step.id, number - 1, step.text);
                      }}
                    >
                      &uarr;
                    </button>
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={`Move ${name} down`}
                      title="Move down"
                      disabled={number === steps.length}
                      aria-disabled={locked}
                      onClick={() => {
                        if (!locked) void actions.moveStep(step.id, number + 1, step.text);
                      }}
                    >
                      &darr;
                    </button>
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={`Edit ${name}`}
                      title="Edit"
                      aria-disabled={locked}
                      onClick={() => {
                        if (!locked) setMode({ kind: 'edit', id: step.id, text: step.text });
                      }}
                    >
                      &#9998;
                    </button>
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={`Remove ${name}`}
                      title="Remove"
                      aria-disabled={locked}
                      onClick={() => {
                        if (!locked) setMode({ kind: 'remove', id: step.id, text: step.text });
                      }}
                    >
                      &times;
                    </button>
                  </span>
                </div>
                {confirming && (
                  <div
                    role="alertdialog"
                    aria-label={`Remove ${name}`}
                    className="actions db-step-confirm"
                    onKeyDown={(event) => {
                      if (event.key !== 'Escape') return;
                      event.stopPropagation();
                      close();
                    }}
                  >
                    <span>
                      Remove {name} &quot;{step.text}&quot;?
                    </span>
                    <Button
                      type="button"
                      variant="danger"
                      className="small"
                      disabled={actions.busy}
                      onClick={() => {
                        void actions.removeStep(step.id, mode.text).then((removed) => {
                          if (removed) close();
                        });
                      }}
                    >
                      Remove
                    </Button>
                    <Button type="button" className="small" autoFocus onClick={close}>
                      Cancel
                    </Button>
                  </div>
                )}
                {step.detail && (
                  <details className="db-step-detail">
                    <summary>Guidance</summary>
                    <div className="db-text">{step.detail}</div>
                  </details>
                )}
              </li>
            );
          })}
        </ol>
      )}
      {mode?.kind === 'add' ? (
        <StepForm
          label="New step"
          busy={actions.busy}
          submitLabel="Add"
          onCancel={close}
          onSave={(value) => {
            void actions.addStep(value.text, guidance(value.detail) ?? undefined).then((added) => {
              if (added) close();
            });
          }}
        />
      ) : (
        <button
          ref={addButton}
          type="button"
          className="btn small db-step-add"
          disabled={locked}
          onClick={() => {
            setMode({ kind: 'add' });
          }}
        >
          + Step
        </button>
      )}
    </section>
  );
}
