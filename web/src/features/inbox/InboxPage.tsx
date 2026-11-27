import { useQuery } from '@tanstack/react-query';
import { useEffect, useMemo, useRef, useState, type DragEvent } from 'react';
import { Link, useSearchParams } from 'react-router';
import { problemMessage, type ProblemMessages } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import { boardsQuery } from '../boards/queries';
import { BOARD_PARAM, NOTES_PARAM, TICKET_PARAM } from '../board/BoardPage';
import '../board/board.css';
import { QUESTION_PARAM } from '../questions/QuestionsPage';
import {
  INBOX_KINDS,
  KIND_LABELS,
  entryKey,
  inboxQuery,
  moveItem,
  useInboxAction,
  useResetOrder,
  useSaveOrder,
  type InboxItem,
  type InboxKind,
} from './queries';
import './inbox.css';
import { Button } from '../../shared/ui/Button';

const ORDER_MESSAGES: ProblemMessages = {
  'version-conflict':
    'The order was changed elsewhere. The inbox is up to date now; move the entry again.',
};

// Kinds from ?kind=, split by commas; unknown ones are dropped.
function kindsOf(raw: string | null): InboxKind[] {
  const names = (raw ?? '').split(',');
  return INBOX_KINDS.filter((kind) => names.includes(kind));
}

// Where an entry leads: the ticket panel, the question, or the board with its notes dialog.
export function inboxTarget(item: InboxItem): string {
  const key = encodeURIComponent(item.key);
  if (item.type === 'question') return `/questions?${QUESTION_PARAM}=${key}`;
  if (item.type === 'ticket') return `/board?${TICKET_PARAM}=${key}`;
  if (item.boardId === null || item.boardId === undefined) return '/board';
  return `/board?${BOARD_PARAM}=${encodeURIComponent(item.boardId)}&${NOTES_PARAM}=1`;
}

function shownKey(item: InboxItem): string {
  return item.type === 'note' ? `Note ${item.key}` : item.key;
}

interface RowProps {
  item: InboxItem;
  index: number;
  count: number;
  reorderable: boolean;
  onDragStart: (key: string) => void;
  onDragEnd: () => void;
  onDrop: (key: string) => void;
  onMove: (key: string, step: -1 | 1) => void;
}

function InboxRow({
  item,
  index,
  count,
  reorderable,
  onDragStart,
  onDragEnd,
  onDrop,
  onMove,
}: RowProps) {
  const action = useInboxAction();
  const [over, setOver] = useState(false);
  const key = entryKey(item);
  const label =
    item.kind === 'accept'
      ? 'Accept'
      : item.kind === 'merge'
        ? 'Retry merge'
        : item.kind === 'note'
          ? 'Make ticket'
          : null;
  const dragProps = reorderable
    ? {
        draggable: true,
        onDragStart: (event: DragEvent<HTMLLIElement>) => {
          event.dataTransfer.setData('text/plain', key);
          onDragStart(key);
        },
        onDragEnd: () => {
          setOver(false);
          onDragEnd();
        },
        onDragOver: (event: DragEvent<HTMLLIElement>) => {
          event.preventDefault();
          setOver(true);
        },
        onDragLeave: () => {
          setOver(false);
        },
        onDrop: (event: DragEvent<HTMLLIElement>) => {
          event.preventDefault();
          setOver(false);
          onDrop(key);
        },
      }
    : {};
  const waiting =
    item.kind === 'question'
      ? ` / ${String(item.waitingTickets)} ${item.waitingTickets === 1 ? 'ticket waits' : 'tickets wait'}`
      : '';
  return (
    <li
      className={`inbox-row kind-${item.kind}${over ? ' drop-target' : ''}`}
      data-key={key}
      {...dragProps}
    >
      <div className="inbox-main">
        <span className="badge inbox-badge">{KIND_LABELS[item.kind]}</span>
        <Link to={inboxTarget(item)} className="inbox-title">
          <span className="card-id">{shownKey(item)}</span> {item.title}
        </Link>
        <span className="muted inbox-meta">
          {item.projectKey}
          {item.boardName === null || item.boardName === undefined ? '' : ` / ${item.boardName}`}
          {waiting}
          {` / since ${new Date(item.sinceAt).toLocaleString()}`}
        </span>
        {item.detail && <p className="inbox-detail muted">{item.detail}</p>}
        {action.data && <p role="status">{action.data}</p>}
        {action.error && <FormError message={action.error.message} />}
      </div>
      <div className="inbox-actions">
        <button
          type="button"
          className="btn"
          data-move="up"
          aria-label={`Move ${shownKey(item)} up`}
          disabled={!reorderable || index === 0}
          onClick={() => {
            onMove(key, -1);
          }}
        >
          Up
        </button>
        <button
          type="button"
          className="btn"
          data-move="down"
          aria-label={`Move ${shownKey(item)} down`}
          disabled={!reorderable || index === count - 1}
          onClick={() => {
            onMove(key, 1);
          }}
        >
          Down
        </button>
        {label && (
          <Button
            type="button"
            disabled={action.isPending}
            onClick={() => {
              action.mutate(item);
            }}
          >
            {label}
          </Button>
        )}
      </div>
    </li>
  );
}

// What waits on the boards; filters in ?board= and ?kind=, reordering only without filters.
export function InboxPage() {
  const [search, setSearch] = useSearchParams();
  const boardId = search.get('board');
  const kinds = kindsOf(search.get('kind'));
  const boards = useQuery(boardsQuery());
  const inbox = useQuery(inboxQuery(boardId));
  const save = useSaveOrder(boardId);
  const reset = useResetOrder();
  const dragged = useRef<string | null>(null);
  const list = useRef<HTMLOListElement>(null);
  const refocus = useRef<{ key: string; button: 'up' | 'down' } | null>(null);
  const [announcement, setAnnouncement] = useState('');

  const data = inbox.data;
  const items = useMemo(() => data?.items ?? [], [data]);
  const shown = kinds.length > 0 ? items.filter((item) => kinds.includes(item.kind)) : items;
  const filtering = boardId !== null || kinds.length > 0;

  // A moved row keeps the keyboard focus on its button, as far as that one still works.
  useEffect(() => {
    const target = refocus.current;
    if (target === null) return;
    refocus.current = null;
    const row = [...(list.current?.children ?? [])].find(
      (child) => child instanceof HTMLElement && child.dataset['key'] === target.key,
    );
    const buttons = [...(row?.querySelectorAll<HTMLButtonElement>('button[data-move]') ?? [])];
    const usable = buttons.filter((button) => !button.disabled);
    const same = usable.find((button) => button.dataset['move'] === target.button);
    (same ?? usable[0])?.focus();
  }, [items]);

  function setParam(key: string, value: string | null) {
    const next = new URLSearchParams(search);
    if (value) next.set(key, value);
    else next.delete(key);
    setSearch(next, { replace: true });
  }

  function toggleKind(kind: InboxKind) {
    const next = kinds.includes(kind) ? kinds.filter((k) => k !== kind) : [...kinds, kind];
    setParam('kind', next.length > 0 ? next.join(',') : null);
  }

  // Moves the entry named by its type and key to the place of another entry.
  function move(from: string, to: string) {
    const next = moveItem(
      items,
      items.findIndex((item) => entryKey(item) === from),
      items.findIndex((item) => entryKey(item) === to),
    );
    if (next === items) return;
    save.mutate(next);
    const moved = next.find((item) => entryKey(item) === from);
    if (moved) {
      const place = String(next.indexOf(moved) + 1);
      setAnnouncement(`${shownKey(moved)} moved to position ${place} of ${String(next.length)}.`);
    }
  }

  function step(key: string, by: -1 | 1) {
    const at = items.findIndex((item) => entryKey(item) === key);
    const neighbour = items[at + by];
    if (!neighbour) return;
    refocus.current = { key, button: by < 0 ? 'up' : 'down' };
    move(key, entryKey(neighbour));
  }

  return (
    <div className="page">
      <div className="db-board-head">
        <h1 className="page-title">Inbox</h1>
      </div>
      <p className="muted">
        What waits for you on the boards. New entries stand on top; you set the order.
      </p>
      <div className="inbox-filters">
        <label className="field">
          <span className="visually-hidden">Board</span>
          <select
            aria-label="Board"
            value={boardId ?? ''}
            onChange={(event) => {
              setParam('board', event.target.value || null);
            }}
          >
            <option value="">All boards</option>
            {boards.data?.map((board) => (
              <option key={board.id} value={board.id}>
                {`${board.projectKey} / ${board.name}`}
              </option>
            ))}
          </select>
        </label>
        {INBOX_KINDS.map((kind) => (
          <button
            key={kind}
            type="button"
            className={kinds.includes(kind) ? 'qx-filter on' : 'qx-filter'}
            aria-pressed={kinds.includes(kind)}
            onClick={() => {
              toggleKind(kind);
            }}
          >
            {data ? `${KIND_LABELS[kind]} (${String(data.counts[kind])})` : KIND_LABELS[kind]}
          </button>
        ))}
        <Button
          type="button"
          disabled={reset.isPending || !data}
          onClick={() => {
            if (data) reset.mutate(data.orderVersion);
          }}
        >
          Reset order
        </Button>
      </div>
      {filtering && shown.length > 1 && (
        <p className="muted inbox-hint">Reordering works only without filters.</p>
      )}
      <p className="visually-hidden" role="status" aria-label="Order changes">
        {announcement}
      </p>
      {save.error && (
        <FormError
          message={`Could not save the order: ${problemMessage(save.error, ORDER_MESSAGES, 'try again.') ?? ''}`}
        />
      )}
      {reset.error && (
        <FormError
          message={problemMessage(reset.error, ORDER_MESSAGES, 'Could not reset the order.') ?? ''}
        />
      )}
      {inbox.status === 'pending' && <p className="muted">Loading the inbox...</p>}
      {inbox.status === 'error' && <p className="form-error">Could not load the inbox.</p>}
      {data && shown.length === 0 && (
        <div className="state empty">
          {filtering ? (
            <h2>Nothing with these filters</h2>
          ) : (
            <>
              <h2>Nothing waits for you</h2>
              <p>
                Blocked tickets, open questions, tickets in planning and open notes of your boards
                show up here.
              </p>
            </>
          )}
        </div>
      )}
      {shown.length > 0 && (
        <ol className="inbox-list" aria-label="Entries" ref={list}>
          {shown.map((item, index) => (
            <InboxRow
              key={entryKey(item)}
              item={item}
              index={index}
              count={shown.length}
              reorderable={!filtering}
              onDragStart={(key) => {
                dragged.current = key;
              }}
              onDragEnd={() => {
                dragged.current = null;
              }}
              onDrop={(key) => {
                if (dragged.current !== null) move(dragged.current, key);
                dragged.current = null;
              }}
              onMove={step}
            />
          ))}
        </ol>
      )}
    </div>
  );
}
