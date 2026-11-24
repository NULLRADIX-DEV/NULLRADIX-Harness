import { useId, useState, type SubmitEvent } from 'react';
import { FormError } from '../../shared/ui/FormError';
import type { Board } from '../boards/queries';
import { PRIORITIES, PRIORITY_LABELS, isPriority, splitList } from '../tickets/labels';
import {
  useCreateTicket,
  type CreateTicketInput,
  type Priority,
  type Ticket,
} from '../tickets/queries';
import { ACCEPTANCE_QUESTION, createProblem, duplicateOf } from './messages';
import { Modal } from './Modal';

interface Draft {
  title: string;
  description: string;
  priority: Priority;
  labels: string;
  acceptance: string;
  dependsOn: string;
  needsPlanning: boolean;
}

const EMPTY: Draft = {
  title: '',
  description: '',
  priority: 'normal',
  labels: '',
  acceptance: '',
  dependsOn: '',
  needsPlanning: true,
};

function inputOf(draft: Draft, allowDuplicate: boolean): CreateTicketInput {
  const description = draft.description.trim();
  const acceptance = draft.acceptance.trim();
  return {
    title: draft.title.trim(),
    priority: draft.priority,
    labels: splitList(draft.labels),
    dependsOn: splitList(draft.dependsOn),
    needsPlanning: draft.needsPlanning,
    ...(description === '' ? {} : { description: draft.description }),
    ...(acceptance === '' ? {} : { acceptance }),
    ...(allowDuplicate ? { allowDuplicate } : {}),
  };
}

// New ticket on a board; with input, cancel and Escape ask before they discard it.
export function NewTicketDialog({
  board,
  onClose,
  onCreated,
}: {
  board: Board;
  onClose: () => void;
  onCreated: (ticket: Ticket) => void;
}) {
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const [asking, setAsking] = useState(false);
  const create = useCreateTicket();
  const titleId = useId();
  const dirty = [draft.title, draft.description, draft.labels, draft.acceptance, draft.dependsOn]
    .map((text) => text.trim())
    .some((text) => text !== '');
  const duplicate = duplicateOf(create.error);

  function edit(change: Partial<Draft>) {
    setDraft((previous) => ({ ...previous, ...change }));
  }

  function send(allowDuplicate: boolean) {
    create.mutate(
      { projectId: board.projectId, boardId: board.id, input: inputOf(draft, allowDuplicate) },
      { onSuccess: onCreated },
    );
  }

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (draft.title.trim() === '' || create.isPending) return;
    send(false);
  }

  function cancel() {
    if (create.isPending) return;
    if (dirty) setAsking(true);
    else onClose();
  }

  return (
    <Modal
      labelledBy={titleId}
      onSubmit={submit}
      onEscape={() => {
        if (asking) setAsking(false);
        else cancel();
      }}
    >
      <h2 id={titleId}>New ticket on {board.name}</h2>
      <label className="field">
        <span>Title</span>
        <input
          value={draft.title}
          autoComplete="off"
          onChange={(event) => {
            edit({ title: event.target.value });
          }}
        />
      </label>
      <label className="field">
        <span>Description</span>
        <textarea
          rows={6}
          value={draft.description}
          onChange={(event) => {
            edit({ description: event.target.value });
          }}
        />
      </label>
      <div className="db-fields">
        <label className="field">
          <span>Priority</span>
          <select
            value={draft.priority}
            onChange={(event) => {
              if (isPriority(event.target.value)) edit({ priority: event.target.value });
            }}
          >
            {PRIORITIES.map((priority) => (
              <option key={priority} value={priority}>
                {PRIORITY_LABELS[priority]}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>Labels</span>
          <input
            value={draft.labels}
            placeholder="comma separated"
            autoComplete="off"
            onChange={(event) => {
              edit({ labels: event.target.value });
            }}
          />
        </label>
        <label className="field">
          <span>Depends on</span>
          <input
            className="mono"
            value={draft.dependsOn}
            placeholder={`${board.projectKey}-0001, ...`}
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => {
              edit({ dependsOn: event.target.value });
            }}
          />
        </label>
        <label className="option-row db-field-check wide">
          <input
            type="checkbox"
            checked={draft.needsPlanning}
            onChange={(event) => {
              edit({ needsPlanning: event.target.checked });
            }}
          />
          <span>Needs planning</span>
        </label>
      </div>
      <label className="field">
        <span>Acceptance</span>
        <textarea
          rows={3}
          placeholder={ACCEPTANCE_QUESTION}
          value={draft.acceptance}
          onChange={(event) => {
            edit({ acceptance: event.target.value });
          }}
        />
      </label>
      <FormError message={createProblem(create.error)} />
      {asking ? (
        <div role="alertdialog" aria-label="Discard input" className="actions">
          <span>Discard the input?</span>
          <button type="button" className="btn danger" onClick={onClose}>
            Discard
          </button>
          <button
            type="button"
            className="btn"
            autoFocus
            onClick={() => {
              setAsking(false);
            }}
          >
            Keep editing
          </button>
        </div>
      ) : (
        <div className="actions">
          <button
            type="submit"
            className="btn primary"
            disabled={create.isPending || draft.title.trim() === ''}
          >
            Create
          </button>
          {duplicate !== null && (
            <button
              type="button"
              className="btn"
              disabled={create.isPending}
              onClick={() => {
                send(true);
              }}
            >
              Create anyway
            </button>
          )}
          <button type="button" className="btn" disabled={create.isPending} onClick={cancel}>
            Cancel
          </button>
        </div>
      )}
    </Modal>
  );
}
