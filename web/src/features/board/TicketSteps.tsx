import { useEffect, useRef, useState } from 'react';
import type { Step } from '../tickets/queries';
import { StepForm } from './StepForm';
import type { TicketActions } from './useTicketActions';

type Mode = { kind: 'add' } | { kind: 'edit' | 'remove'; number: number; text: string } | null;

// Number of the step an open form belongs to after a change of the list; 0 when it is gone.
function locate(steps: readonly Step[], number: number, text: string): number {
  if (steps[number - 1]?.text === text) return number;
  return steps.findIndex((step) => step.text === text) + 1;
}

function guidance(detail: string): string | null {
  return detail.trim() === '' ? null : detail;
}

function StepsProgress({ done, total }: { done: number; total: number }) {
  return (
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
  const target = mode && mode.kind !== 'add' ? locate(steps, mode.number, mode.text) : 0;
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
                        .changeStep(number, {
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
              <li
                key={`${String(number)}:${step.text}`}
                className={step.done ? 'db-step done' : 'db-step'}
              >
                <div className="db-step-line">
                  <label className="db-step-row">
                    <input
                      type="checkbox"
                      checked={step.done}
                      disabled={actions.busy}
                      aria-label={`Step ${String(number)}: ${step.text}`}
                      onChange={(event) => {
                        void actions.changeStep(number, {
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
                      disabled={locked || number === 1}
                      onClick={() => void actions.moveStep(number, number - 1, step.text)}
                    >
                      &uarr;
                    </button>
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={`Move ${name} down`}
                      title="Move down"
                      disabled={locked || number === steps.length}
                      onClick={() => void actions.moveStep(number, number + 1, step.text)}
                    >
                      &darr;
                    </button>
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={`Edit ${name}`}
                      title="Edit"
                      disabled={locked}
                      onClick={() => {
                        setMode({ kind: 'edit', number, text: step.text });
                      }}
                    >
                      &#9998;
                    </button>
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label={`Remove ${name}`}
                      title="Remove"
                      disabled={locked}
                      onClick={() => {
                        setMode({ kind: 'remove', number, text: step.text });
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
                    <button
                      type="button"
                      className="btn danger small"
                      disabled={actions.busy}
                      onClick={() => {
                        void actions.removeStep(number, mode.text).then((removed) => {
                          if (removed) close();
                        });
                      }}
                    >
                      Remove
                    </button>
                    <button type="button" className="btn small" autoFocus onClick={close}>
                      Cancel
                    </button>
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
