import { useState, type KeyboardEvent } from 'react';
import type { Ticket } from '../tickets/queries';
import type { TicketActions } from './useTicketActions';
import { Button } from '../../shared/ui/Button';

export type TextField = 'description' | 'acceptance' | 'plan' | 'handbook';

// `from` is the saved text the draft started from.
interface Edit {
  draft: string;
  from: string;
}

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
  const [edit, setEdit] = useState<Edit | null>(null);
  const text = ticket[field];

  async function save() {
    if (edit === null || actions.busy) return;
    if (edit.draft !== text && text !== edit.from) {
      setEdit({ ...edit, from: text });
      actions.conflict(label);
      return;
    }
    const change: Partial<Record<TextField, string>> = {};
    change[field] = edit.draft;
    if (await actions.save(change)) setEdit(null);
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === 'Escape') {
      // keep the panel open; only an unchanged draft leaves edit mode
      event.stopPropagation();
      if (edit?.draft === text) setEdit(null);
    } else if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      void save();
    }
  }

  if (edit !== null) {
    return (
      <>
        <textarea
          aria-label={label}
          rows={14}
          spellCheck={false}
          value={edit.draft}
          autoFocus
          onChange={(event) => {
            setEdit({ ...edit, draft: event.target.value });
          }}
          onKeyDown={onKeyDown}
        />
        <div className="actions">
          <Button
            type="button"
            variant="primary"
            disabled={actions.busy || edit.draft === text}
            onClick={() => void save()}
          >
            Save (Ctrl+Enter)
          </Button>
          <Button
            type="button"
            onClick={() => {
              setEdit(null);
            }}
          >
            Cancel
          </Button>
        </div>
      </>
    );
  }
  return (
    <>
      <div className="actions">
        <Button
          type="button"
          onClick={() => {
            setEdit({ draft: text, from: text });
          }}
        >
          Edit
        </Button>
      </div>
      {text.trim() === '' ? (
        <p className="muted">{empty}</p>
      ) : (
        <div className="db-text">{text}</div>
      )}
    </>
  );
}
