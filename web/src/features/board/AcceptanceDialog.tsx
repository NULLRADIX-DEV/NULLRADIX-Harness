import { useId, useState } from 'react';
import { ACCEPTANCE_QUESTION } from './messages';
import { Modal } from './Modal';
import { Input } from '../../shared/ui/Input';
import { Button } from '../../shared/ui/Button';

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
      <Input
        multiline
        aria-label="Acceptance"
        rows={4}
        value={text}
        onChange={(text) => {
          setText(text);
        }}
      />
      <div className="actions">
        <Button type="submit" variant="primary" disabled={!ready}>
          Move to done
        </Button>
        <Button type="button" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </Modal>
  );
}
