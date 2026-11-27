import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { useId, useRef, useState, type SubmitEvent } from 'react';
import { ApiProblemError, type SimilarNote } from '../../shared/api/errors';
import { problemMessage, type ProblemMessages } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import { Modal } from '../board/Modal';
import type { Board } from '../boards/queries';
import { splitList } from '../tickets/labels';
import {
  notesQuery,
  useAddNote,
  useDropNote,
  usePromoteNote,
  useSeenNote,
  type Note,
  type NoteFilter,
} from './queries';
import './notes.css';
import { Button } from '../../shared/ui/Button';
import { Input } from '../../shared/ui/Input';

const FILTERS: readonly { key: NoteFilter; label: string }[] = [
  { key: 'open', label: 'Open' },
  { key: 'promoted', label: 'Promoted' },
  { key: 'dropped', label: 'Dropped' },
  { key: 'all', label: 'All' },
];

const TITLE_MAX = 200;

const NOTE_MESSAGES: ProblemMessages = {
  'note-not-found': 'This note no longer exists.',
  'note-not-open': 'This note is no longer open. The list is up to date now.',
  'note-promoted': 'This note became a ticket in the meantime. The list is up to date now.',
  'title-empty': 'The title must not be empty.',
  'reason-missing': 'Nothing is dropped without a reason.',
};

// Title of a note; notes without one show the first filled line of their body.
export function noteTitle(note: Note): string {
  const first = note.body
    .split('\n')
    .map((line) => line.trim())
    .find((line) => line !== '');
  const title = note.title.trim();
  return (title === '' ? (first ?? `Note ${String(note.number)}`) : title).slice(0, TITLE_MAX);
}

// The notes a `note-similar` refusal names; null for any other error.
function similarOf(error: unknown): readonly SimilarNote[] | null {
  if (error instanceof ApiProblemError && error.code === 'note-similar') return error.similar;
  return null;
}

interface Promoting {
  number: number;
  title: string;
  initial: string;
}

interface Dropping {
  number: number;
  reason: string;
}

// Notes of a board: filter, add, promote, drop with a reason; unsaved input is asked about.
export function NotesDialog({
  board,
  onClose,
  onOpenTicket,
}: {
  board: Board;
  onClose: () => void;
  onOpenTicket: (key: string) => void;
}) {
  const [filter, setFilter] = useState<NoteFilter>('open');
  const notes = useQuery({
    ...notesQuery(board.projectId, board.id, filter),
    placeholderData: keepPreviousData,
  });
  const add = useAddNote();
  const seen = useSeenNote();
  const promote = usePromoteNote();
  const drop = useDropNote();
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [labels, setLabels] = useState('');
  const [similar, setSimilar] = useState<readonly SimilarNote[] | null>(null);
  const [promoting, setPromoting] = useState<Promoting | null>(null);
  const [dropping, setDropping] = useState<Dropping | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [leaving, setLeaving] = useState<(() => void) | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);
  const titleId = useId();
  const busy = add.isPending || seen.isPending || promote.isPending || drop.isPending;
  const dirty =
    title.trim() !== '' ||
    body.trim() !== '' ||
    labels.trim() !== '' ||
    (dropping?.reason.trim() ?? '') !== '' ||
    (promoting !== null && promoting.title.trim() !== promoting.initial.trim());
  const list = notes.data ?? [];

  function leave(action: () => void) {
    if (dirty) setLeaving(() => action);
    else action();
  }

  function fail(cause: unknown) {
    const found = similarOf(cause);
    if (found) setSimilar(found);
    else setError(problemMessage(cause, NOTE_MESSAGES, 'Could not change the notes.'));
  }

  function reset() {
    setTitle('');
    setBody('');
    setLabels('');
    setSimilar(null);
    titleRef.current?.focus();
  }

  function send(force: boolean) {
    if (title.trim() === '' || busy) return;
    setError(null);
    const input = { title: title.trim(), body, labels: splitList(labels), force };
    add.mutate(
      { projectId: board.projectId, boardId: board.id, input },
      { onSuccess: reset, onError: fail },
    );
  }

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    send(false);
  }

  function countSeen(number: number) {
    setError(null);
    seen.mutate(number, { onSuccess: reset, onError: fail });
  }

  function sendPromotion() {
    if (!promoting || busy) return;
    if (promoting.title.trim() === '') {
      setError('The title must not be empty.');
      return;
    }
    setError(null);
    promote.mutate(
      { number: promoting.number, input: { title: promoting.title.trim() } },
      {
        onSuccess: () => {
          setPromoting(null);
        },
        onError: fail,
      },
    );
  }

  function sendDrop() {
    if (!dropping || busy) return;
    if (dropping.reason.trim() === '') {
      setError('Nothing is dropped without a reason.');
      return;
    }
    setError(null);
    drop.mutate(
      { number: dropping.number, reason: dropping.reason.trim() },
      {
        onSuccess: () => {
          setDropping(null);
        },
        onError: fail,
      },
    );
  }

  function open(key: string) {
    leave(() => {
      onOpenTicket(key);
    });
  }

  // Closes an open promote or drop form; false when none was open.
  function cancelInline(): boolean {
    if (!promoting && !dropping) return false;
    setPromoting(null);
    setDropping(null);
    setError(null);
    return true;
  }

  const loadError = notes.isError ? 'Could not load the notes.' : null;

  return (
    <Modal
      labelledBy={titleId}
      className="panel db-modal db-notes-dialog"
      onSubmit={submit}
      onEscape={() => {
        if (leaving) setLeaving(null);
        else if (!cancelInline()) leave(onClose);
      }}
    >
      <div className="db-notes-head">
        <h2 id={titleId}>Notes on {board.name}</h2>
        <button
          type="button"
          className="icon-btn"
          aria-label="Close notes"
          onClick={() => {
            leave(onClose);
          }}
        >
          x
        </button>
      </div>
      {leaving && (
        <div role="alertdialog" aria-label="Discard input" className="actions">
          <span>Discard the input?</span>
          <Button
            type="button"
            variant="danger"
            onClick={() => {
              const action = leaving;
              setLeaving(null);
              action();
            }}
          >
            Discard
          </Button>
          <Button
            type="button"
            autoFocus
            onClick={() => {
              setLeaving(null);
            }}
          >
            Keep editing
          </Button>
        </div>
      )}
      <div className="chips" role="group" aria-label="Filter">
        {FILTERS.map((each) => (
          <button
            key={each.key}
            type="button"
            className="btn db-note-filter"
            aria-pressed={filter === each.key}
            onClick={() => {
              setFilter(each.key);
            }}
          >
            {each.label}
          </button>
        ))}
      </div>
      <label className="field">
        <span>New note</span>
        <input
          ref={titleRef}
          value={title}
          maxLength={TITLE_MAX}
          autoComplete="off"
          placeholder="What stands out? One sentence that makes sense without context"
          onChange={(event) => {
            setTitle(event.target.value);
            setSimilar(null);
          }}
        />
      </label>
      <label className="field">
        <span>Details</span>
        <Input
          multiline
          rows={3}
          value={body}
          placeholder="Where, how to see it, suspected cause, proposal"
          onChange={(text) => {
            setBody(text);
            setSimilar(null);
          }}
        />
      </label>
      <label className="field">
        <span>Labels of the note</span>
        <input
          value={labels}
          placeholder="comma separated"
          autoComplete="off"
          onChange={(event) => {
            setLabels(event.target.value);
          }}
        />
      </label>
      {similar ? (
        <div role="alert" className="db-note-similar">
          <p>This finding probably exists already:</p>
          <ul>
            {similar.map((each) => (
              <li key={each.number}>
                {each.title}{' '}
                {each.state === 'open' && (
                  <Button
                    type="button"
                    disabled={busy}
                    onClick={() => {
                      countSeen(each.number);
                    }}
                  >
                    Same finding: count it
                  </Button>
                )}
                {each.state !== 'open' && each.ticketKey && (
                  <button
                    type="button"
                    className="link-btn"
                    onClick={() => {
                      if (each.ticketKey) open(each.ticketKey);
                    }}
                  >
                    already {each.ticketKey}
                  </button>
                )}
              </li>
            ))}
          </ul>
          <div className="actions">
            <Button
              type="button"
              disabled={busy}
              onClick={() => {
                send(true);
              }}
            >
              Add anyway
            </Button>
          </div>
        </div>
      ) : (
        <div className="actions">
          <Button type="submit" variant="primary" disabled={title.trim() === '' || busy}>
            Add note
          </Button>
        </div>
      )}
      <FormError message={error ?? loadError} />
      {notes.isSuccess && list.length === 0 && (
        <p className="muted">
          {filter === 'open'
            ? 'No open notes. What stands out while working and does not belong to the current ticket goes here.'
            : 'Nothing in this state.'}
        </p>
      )}
      <ul className="db-notes" aria-label="Notes" aria-busy={notes.isFetching}>
        {list.map((note) => (
          <li key={note.number} className="db-note">
            <div className="chips">
              <span className="tag mono">#{note.number}</span>
              {note.state === 'open' && <span className="badge waiting">open</span>}
              {note.state === 'promoted' && note.ticketKey && (
                <span className="badge">
                  promoted to{' '}
                  <button
                    type="button"
                    className="link-btn"
                    onClick={() => {
                      if (note.ticketKey) open(note.ticketKey);
                    }}
                  >
                    {note.ticketKey}
                  </button>
                </span>
              )}
              {note.state === 'dropped' && (
                <span className="badge blocked">dropped: {note.reason}</span>
              )}
              {note.source && (
                <button
                  type="button"
                  className="link-btn"
                  onClick={() => {
                    if (note.source) open(note.source);
                  }}
                >
                  from {note.source}
                </button>
              )}
              {note.seenCount > 1 && (
                <span
                  className="badge"
                  title={note.seenIn.length > 0 ? `also at ${note.seenIn.join(', ')}` : undefined}
                >
                  seen {note.seenCount} times
                </span>
              )}
              {note.labels.map((label) => (
                <span key={label} className="tag">
                  {label}
                </span>
              ))}
              <span className="muted">
                {note.authorName}, {new Date(note.createdAt).toLocaleString()}
              </span>
            </div>
            <strong className="db-note-title">{noteTitle(note)}</strong>
            {note.body.trim() !== '' && <p className="db-note-body">{note.body}</p>}
            {note.state === 'open' && promoting?.number === note.number && (
              <div className="actions">
                <label className="field">
                  <span>Ticket title</span>
                  <Input
                    value={promoting.title}
                    autoFocus
                    onChange={(text) => {
                      setPromoting({ ...promoting, title: text });
                    }}
                  />
                </label>
                <Button type="button" variant="primary" disabled={busy} onClick={sendPromotion}>
                  Create ticket
                </Button>
                <Button
                  type="button"
                  disabled={busy}
                  onClick={() => {
                    setPromoting(null);
                  }}
                >
                  Cancel
                </Button>
              </div>
            )}
            {note.state === 'open' && dropping?.number === note.number && (
              <div className="actions">
                <label className="field">
                  <span>Reason</span>
                  <Input
                    value={dropping.reason}
                    autoFocus
                    onChange={(text) => {
                      setDropping({ number: note.number, reason: text });
                    }}
                  />
                </label>
                <Button type="button" variant="danger" disabled={busy} onClick={sendDrop}>
                  Confirm drop
                </Button>
                <Button
                  type="button"
                  disabled={busy}
                  onClick={() => {
                    setDropping(null);
                  }}
                >
                  Cancel
                </Button>
              </div>
            )}
            {note.state === 'open' &&
              promoting?.number !== note.number &&
              dropping?.number !== note.number && (
                <div className="actions">
                  <Button
                    type="button"
                    variant="primary"
                    onClick={() => {
                      setDropping(null);
                      const suggestion = noteTitle(note);
                      setPromoting({ number: note.number, title: suggestion, initial: suggestion });
                    }}
                  >
                    Make a ticket
                  </Button>
                  <Button
                    type="button"
                    onClick={() => {
                      setPromoting(null);
                      setDropping({ number: note.number, reason: '' });
                    }}
                  >
                    Drop
                  </Button>
                </div>
              )}
          </li>
        ))}
      </ul>
    </Modal>
  );
}
