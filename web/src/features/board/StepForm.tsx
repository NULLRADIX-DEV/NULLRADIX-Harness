import { useState, type KeyboardEvent, type SubmitEvent } from 'react';

export const STEP_MAX_LENGTH = 1000;

export interface StepValue {
  text: string;
  detail: string;
}

// Text and optional guidance of a step; Enter in the text saves, Escape cancels.
export function StepForm({
  label,
  initial = { text: '', detail: '' },
  busy,
  submitLabel,
  onSave,
  onCancel,
}: {
  label: string;
  initial?: StepValue;
  busy: boolean;
  submitLabel: string;
  onSave: (value: StepValue) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initial.text);
  const [detail, setDetail] = useState(initial.detail);
  const empty = text.trim() === '';

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (empty || busy) return;
    onSave({ text: text.trim(), detail });
  }

  function onKeyDown(event: KeyboardEvent<HTMLFormElement>) {
    if (event.key !== 'Escape') return;
    // Escape leaves the form, not the ticket panel
    event.preventDefault();
    event.stopPropagation();
    onCancel();
  }

  return (
    <form className="db-step-form" aria-label={label} onSubmit={submit} onKeyDown={onKeyDown}>
      <label className="field">
        <span>Text</span>
        <input
          value={text}
          autoFocus
          autoComplete="off"
          maxLength={STEP_MAX_LENGTH}
          onChange={(event) => {
            setText(event.target.value);
          }}
        />
      </label>
      <label className="field">
        <span>Guidance (optional)</span>
        <textarea
          rows={3}
          value={detail}
          onChange={(event) => {
            setDetail(event.target.value);
          }}
        />
      </label>
      <div className="actions">
        <button type="submit" className="btn primary small" disabled={empty || busy}>
          {submitLabel}
        </button>
        <button type="button" className="btn small" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}
