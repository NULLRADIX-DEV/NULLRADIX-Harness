import { useState, type KeyboardEvent } from 'react';
import type { Ticket } from '../tickets/queries';
import type { TicketActions } from './useTicketActions';

export type TextField = 'description' | 'acceptance' | 'plan' | 'handbook';

// A long text of a ticket: shown as written, edited in place and saved with Ctrl+Enter.
export function TextTab({
  ticket,
  field,
  label,
  empty,
  actions,
}: {
  ticket: Ticket;
  field: TextField;
  label: string;
  empty: string;
  actions: TicketActions;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const text = ticket[field];

  async function save() {
    if (draft === null || actions.busy) return;
    const change: Partial<Record<TextField, string>> = {};
    change[field] = draft;
    if (await actions.save(change)) setDraft(null);
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      void save();
    }
  }

  if (draft !== null) {
    return (
      <>
        <textarea
          aria-label={label}
          rows={14}
          spellCheck={false}
          value={draft}
          autoFocus
          onChange={(event) => {
            setDraft(event.target.value);
          }}
          onKeyDown={onKeyDown}
        />
        <div className="actions">
          <button
            type="button"
            className="btn primary"
            disabled={actions.busy || draft === text}
            onClick={() => void save()}
          >
            Save (Ctrl+Enter)
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              setDraft(null);
            }}
          >
            Cancel
          </button>
        </div>
      </>
    );
  }
  return (
    <>
      <div className="actions">
        <button
          type="button"
          className="btn"
          onClick={() => {
            setDraft(text);
          }}
        >
          Edit
        </button>
      </div>
      {text.trim() === '' ? (
        <p className="muted">{empty}</p>
      ) : (
        <div className="db-text">{text}</div>
      )}
    </>
  );
}
