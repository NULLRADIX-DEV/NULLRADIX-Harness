import { useId, useState, type KeyboardEvent } from 'react';
import type { Comment } from '../tickets/queries';
import { dayLabel } from './TicketHistory';
import type { TicketActions } from './useTicketActions';
import { useNow } from '../../shared/lib/useNow';
import { Button } from '../../shared/ui/Button';

const TIME = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });
const FULL = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'medium' });

// Two letters for the avatar: the start of two words, else of the one ("Florian" -> "FL").
export function initials(name: string): string {
  const words = name
    .trim()
    .split(/[\s._-]+/)
    .filter((word) => word !== '');
  const [first, second] = words;
  if (first === undefined) return '?';
  const letters = second === undefined ? first.slice(0, 2) : `${first[0] ?? ''}${second[0] ?? ''}`;
  return letters.toUpperCase();
}

// Text box with the send button below; Ctrl+Enter (Cmd+Enter) sends, blank text never.
function Composer({ actions }: { actions: TicketActions }) {
  const [body, setBody] = useState('');
  const id = useId();
  const empty = body.trim() === '';

  function send() {
    if (empty || actions.busy) return;
    void actions.comment(body).then((saved) => {
      if (saved) setBody('');
    });
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== 'Enter' || !(event.ctrlKey || event.metaKey)) return;
    event.preventDefault();
    send();
  }

  return (
    <div className="db-composer">
      <div className="db-composer-box">
        <label className="visually-hidden" htmlFor={id}>
          New comment
        </label>
        <textarea
          id={id}
          rows={3}
          placeholder="Write a comment"
          value={body}
          onChange={(event) => {
            setBody(event.target.value);
          }}
          onKeyDown={onKeyDown}
        />
        <div className="db-composer-actions">
          <span className="muted">Ctrl+Enter</span>
          <Button
            type="button"
            variant="primary"
            className="small"
            disabled={empty || actions.busy}
            onClick={send}
          >
            Comment
          </Button>
        </div>
      </div>
    </div>
  );
}

// Comments of a ticket, newest first, under a box for a new one.
export function TicketComments({
  comments,
  actions,
}: {
  comments: readonly Comment[];
  actions: TicketActions;
}) {
  const now = useNow(false);
  return (
    <section className="db-comments" aria-label="Comments">
      <Composer actions={actions} />
      {comments.length === 0 ? (
        <p className="muted">No comments yet.</p>
      ) : (
        <ol className="db-comment-list">
          {[...comments].reverse().map((comment) => (
            <li key={comment.id} className="db-comment">
              <span className="db-avatar" aria-hidden="true">
                {initials(comment.authorName)}
              </span>
              <div className="db-comment-main">
                <div className="db-comment-head">
                  <strong>{comment.authorName}</strong>
                  {comment.authorKind !== 'human' && <span className="muted">agent</span>}
                  <time
                    className="tl-time"
                    dateTime={new Date(comment.createdAt).toISOString()}
                    title={FULL.format(comment.createdAt)}
                  >
                    {dayLabel(comment.createdAt, now)} {TIME.format(comment.createdAt)}
                  </time>
                </div>
                <p className="db-comment-body">{comment.body}</p>
              </div>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
