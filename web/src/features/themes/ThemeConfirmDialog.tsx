import { useId } from 'react';
import { ApiProblemError, type Confirmation } from '../../shared/api/errors';
import '../board/board.css';
import { Modal } from '../board/Modal';

export const CONFIRMATION_CODE = 'theme-confirmation-required';

// The facts of a refused install that waits for a confirmation, or null for any other state.
export function confirmationOf(error: unknown): Confirmation | null {
  if (!(error instanceof ApiProblemError) || error.code !== CONFIRMATION_CODE) return null;
  return error.confirmation;
}

// Asks before a package with code or network domains is installed; cancel installs nothing.
export function ThemeConfirmDialog({
  name,
  facts,
  busy,
  onConfirm,
  onCancel,
}: {
  name: string;
  facts: Confirmation;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const titleId = useId();
  const descriptionId = useId();
  const update = facts.update;
  return (
    <Modal
      labelledBy={titleId}
      describedBy={descriptionId}
      className="panel db-dialog"
      onEscape={onCancel}
      onSubmit={(event) => {
        event.preventDefault();
        if (!busy) onConfirm();
      }}
    >
      <strong id={titleId}>{update ? `Update ${name}?` : `Install ${name}?`}</strong>
      <div id={descriptionId} className="theme-confirm-facts">
        {facts.codeAdded && (
          <p>
            {update ? 'This update adds code.' : 'This theme contains code.'} Code runs in NULLRADIX
            with full rights. Install it only from a source you trust.
          </p>
        )}
        {facts.domainsAdded.length > 0 && (
          <>
            <p>
              {update
                ? 'This update adds network domains:'
                : 'This theme declares network domains:'}
            </p>
            <ul aria-label="Network domains">
              {facts.domainsAdded.map((domain) => (
                <li key={domain}>{domain}</li>
              ))}
            </ul>
          </>
        )}
        {facts.claudeTheme && (
          <p>It also brings a Claude Code theme that is put into the Claude folders.</p>
        )}
      </div>
      <div className="actions">
        <button type="submit" className="btn primary" disabled={busy}>
          {update ? 'Update' : 'Install'}
        </button>
        <button type="button" className="btn" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </Modal>
  );
}
