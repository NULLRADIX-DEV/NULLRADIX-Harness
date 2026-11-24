import { useId, useState } from 'react';
import { ACCEPTANCE_QUESTION } from './messages';
import { Modal } from './Modal';

// Asks for the missing acceptance criteria before a move to done; cancel leaves the ticket.
export function AcceptanceDialog({
  ticketKey,
  onConfirm,
  onCancel,
}: {
  ticketKey: string;
  onConfirm: (acceptance: string) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState('');
  const titleId = useId();
  const ready = text.trim() !== '';
  return (
    <Modal
      labelledBy={titleId}
      className="panel db-dialog"
      onEscape={onCancel}
      onSubmit={(event) => {
        event.preventDefault();
        if (ready) onConfirm(text.trim());
      }}
    >
      <strong id={titleId}>{ACCEPTANCE_QUESTION}</strong>
      <p className="db-dialog-hint">
        {ticketKey} has no acceptance criteria yet. It cannot move to done without them.
      </p>
      <textarea
        aria-label="Acceptance"
        rows={4}
        value={text}
        onChange={(event) => {
          setText(event.target.value);
        }}
      />
      <div className="actions">
        <button type="submit" className="btn primary" disabled={!ready}>
          Move to done
        </button>
        <button type="button" className="btn" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </Modal>
  );
}
