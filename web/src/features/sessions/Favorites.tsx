import { useQuery } from '@tanstack/react-query';
import { useState, type SubmitEvent } from 'react';
import { Link } from 'react-router';
import { formatAgo } from './format';
import {
  favoritesQuery,
  useMarkFavorite,
  useUnmarkFavorite,
  type Favorite,
} from './favoriteQueries';
import { SessionStateBadge } from './SessionStateBadge';

const SHORT_ID = 8;

// The star of a session: shows whether it is a favorite and toggles it.
export function FavoriteStar({ sessionId }: { sessionId: string }) {
  const { data } = useQuery(favoritesQuery());
  const mark = useMarkFavorite();
  const unmark = useUnmarkFavorite();
  const active = (data ?? []).some((favorite) => favorite.sessionId === sessionId);
  const busy = mark.isPending || unmark.isPending;
  return (
    <button
      type="button"
      className={active ? 'star on' : 'star'}
      aria-pressed={active}
      aria-label={active ? 'Remove from favorites' : 'Add to favorites'}
      disabled={busy}
      onClick={() => {
        if (active) unmark.mutate(sessionId);
        else mark.mutate({ sessionId });
      }}
    >
      <svg width="18" height="18" viewBox="0 0 24 24" strokeWidth="1.5" aria-hidden="true">
        <path d="M12 3l2.7 5.6 6.1.8-4.5 4.2 1.1 6.1L12 16.8 6.6 19.7l1.1-6.1L3.2 9.4l6.1-.8z" />
      </svg>
    </button>
  );
}

function CopyCommand({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="fav-command">
      <code className="mono">{command}</code>
      <button
        type="button"
        className="btn small"
        onClick={() => {
          void navigator.clipboard.writeText(command).then(() => {
            setCopied(true);
          });
        }}
      >
        {copied ? 'Copied' : 'Copy'}
      </button>
    </div>
  );
}

// One favorite: note, tickets, folder, state, resumptions and the command that resumes it.
function FavoriteItem({ favorite, now }: { favorite: Favorite; now: number }) {
  const [note, setNote] = useState(favorite.note);
  const mark = useMarkFavorite();
  const unmark = useUnmarkFavorite();
  const short = favorite.sessionId.slice(0, SHORT_ID);

  function save(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    mark.mutate({ sessionId: favorite.sessionId, note });
  }

  return (
    <article className="panel fav-card" aria-label={`Favorite ${short}`}>
      <div className="fav-head">
        <SessionStateBadge state={favorite.displayState} />
        <Link to={`/sessions/${encodeURIComponent(favorite.sessionId)}`}>
          {favorite.label === '' ? short : favorite.label}
        </Link>
        {favorite.tickets.map((key) => (
          <Link key={key} className="mono" to={`/board?ticket=${key}`}>
            {key}
          </Link>
        ))}
      </div>
      <form className="fav-note" onSubmit={save}>
        <label>
          <span className="muted">Note</span>
          <input
            value={note}
            maxLength={2000}
            placeholder="Why does this session matter?"
            onChange={(event) => {
              setNote(event.target.value);
            }}
          />
        </label>
        <button type="submit" className="btn small" disabled={mark.isPending}>
          Save note
        </button>
        {mark.isSuccess && <span role="status">Saved</span>}
      </form>
      <div className="fav-meta muted">
        <span>Last activity {formatAgo(favorite.lastEventAt, now)}</span>
        <span className="mono">{favorite.cwd}</span>
        {!favorite.live && <span>(no longer in the session list)</span>}
      </div>
      {!favorite.cwdExists && (
        <p role="alert" className="form-error">
          The folder is gone. The command works again once it exists, such as in a new worktree.
        </p>
      )}
      <CopyCommand command={favorite.command} />
      {favorite.resumedAs.length > 0 && (
        <p className="muted">
          Resumed as{' '}
          {favorite.resumedAs
            .map((resumption) => resumption.sessionId.slice(0, SHORT_ID))
            .join(', ')}
        </p>
      )}
      <button
        type="button"
        className="btn small"
        disabled={unmark.isPending}
        onClick={() => {
          unmark.mutate(favorite.sessionId);
        }}
      >
        Remove favorite
      </button>
    </article>
  );
}

// Marked sessions, kept past the session list with the command that resumes them.
export function FavoritesSection({ now }: { now: number }) {
  const { status, data } = useQuery(favoritesQuery());
  if (status !== 'success' || data.length === 0) return null;
  return (
    <section className="session-favorites" aria-label="Favorites">
      <h2>Favorites</h2>
      {data.map((favorite) => (
        <FavoriteItem key={favorite.sessionId} favorite={favorite} now={now} />
      ))}
    </section>
  );
}
