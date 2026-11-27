import { useId } from 'react';
import { ApiProblemError, type Confirmation } from '../../shared/api/errors';
import '../board/board.css';
import { Modal } from '../board/Modal';
import { Button } from '../../shared/ui/Button';

export const CONFIRMATION_CODE = 'theme-confirmation-required';

// The facts of a refused install that waits for a confirmation, or null for any other state.
export function confirmationOf(error: unknown): Confirmation | null {
  if (!(error instanceof ApiProblemError) || error.code !== CONFIRMATION_CODE) return null;
  return error.confirmation;
}

// What happens to the code switch of the theme after this install.
function codeSwitchLine(facts: Confirmation): string {
  if (!facts.update) return 'The code switch of this theme starts off.';
  const added = facts.added.code || facts.added.claudeTheme || facts.added.domains.length > 0;
  return added
    ? 'This update adds something, so the code switch of this theme turns off until you turn it on again.'
    : 'This update adds nothing new, so the code switch of this theme stays as it is.';
}

// Asks before a package with code, network domains or a Claude part is installed.
export function ThemeConfirmDialog({
  facts,
  busy,
  onConfirm,
  onCancel,
}: {
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
      role="alertdialog"
      className="panel db-dialog"
      onEscape={onCancel}
      onSubmit={(event) => {
        event.preventDefault();
        if (!busy) onConfirm();
      }}
    >
      <strong id={titleId}>
        {update ? 'Update' : 'Install'} {facts.name} {facts.version}?
      </strong>
      <div id={descriptionId} className="theme-confirm-facts">
        {facts.hasCode && (
          <p>
            This package contains code
            {update && facts.added.code ? ' (new in this update)' : ''}. Code runs in NULLRADIX with
            full rights. Install it only from a source you trust.
          </p>
        )}
        {facts.hasCode && <p>{codeSwitchLine(facts)}</p>}
        {facts.claudeTheme && (
          <p>
            It has a Claude Code theme that is put into the Claude folders
            {update && facts.added.claudeTheme ? ' (new in this update)' : ''}.
          </p>
        )}
        {facts.domains.length > 0 && (
          <>
            <p>It declares network domains:</p>
            <ul aria-label="Network domains">
              {facts.domains.map((domain) => (
                <li key={domain}>
                  {domain}
                  {update && facts.added.domains.includes(domain) ? ' (new)' : ''}
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
      <div className="actions">
        <Button type="button" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={busy}>
          {update ? 'Update' : 'Install'}
        </Button>
      </div>
    </Modal>
  );
}
