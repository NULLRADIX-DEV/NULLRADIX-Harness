import { useQuery } from '@tanstack/react-query';
import { useRef, useState, type DragEvent } from 'react';
import { Link, useSearchParams } from 'react-router';
import { FormError } from '../../shared/ui/FormError';
import { boardsQuery } from '../boards/queries';
import { TICKET_PARAM } from '../board/BoardPage';
import '../board/board.css';
import { QUESTION_PARAM } from '../questions/QuestionsPage';
import {
  INBOX_KINDS,
  KIND_LABELS,
  inboxQuery,
  moveItem,
  useInboxAction,
  useResetOrder,
  useSaveOrder,
  type InboxItem,
  type InboxKind,
} from './queries';
import './inbox.css';

// Kinds from ?kind=, split by commas; unknown ones are dropped.
function kindsOf(raw: string | null): InboxKind[] {
  const names = (raw ?? '').split(',');
  return INBOX_KINDS.filter((kind) => names.includes(kind));
}

// Where an entry leads: the ticket panel, the question, or the board with its notes.
export function inboxTarget(item: InboxItem): string {
  const key = encodeURIComponent(item.key);
  if (item.type === 'question') return `/questions?${QUESTION_PARAM}=${key}`;
  if (item.type === 'ticket') return `/board?${TICKET_PARAM}=${key}`;
  return '/board';
}

function shownKey(item: InboxItem): string {
  return item.type === 'note' ? `Note ${item.key}` : item.key;
}

interface RowProps {
  item: InboxItem;
  index: number;
  draggable: boolean;
  onDragStart: (index: number) => void;
  onDrop: (index: number) => void;
}

function InboxRow({ item, index, draggable, onDragStart, onDrop }: RowProps) {
  const action = useInboxAction();
  const [over, setOver] = useState(false);
  const label = item.kind === 'accept' ? 'Accept' : item.kind === 'note' ? 'Make ticket' : null;
  const dragProps = draggable
    ? {
        draggable: true,
        onDragStart: (event: DragEvent<HTMLLIElement>) => {
          event.dataTransfer.setData('text/plain', `${item.type}:${item.key}`);
          onDragStart(index);
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
          onDrop(index);
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
      data-key={`${item.type}:${item.key}`}
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
        {action.error && <FormError message={action.error.message} />}
      </div>
      {label && (
        <div className="inbox-actions">
          <button
            type="button"
            className="btn"
            disabled={action.isPending}
            onClick={() => {
              action.mutate(item);
            }}
          >
            {label}
          </button>
        </div>
      )}
    </li>
  );
}

// What waits on the boards; filters in ?board= and ?kind=, dragging only without filters.
export function InboxPage() {
  const [search, setSearch] = useSearchParams();
  const boardId = search.get('board');
  const kinds = kindsOf(search.get('kind'));
  const boards = useQuery(boardsQuery());
  const inbox = useQuery(inboxQuery(boardId));
  const save = useSaveOrder(boardId);
  const reset = useResetOrder();
  const dragFrom = useRef<number | null>(null);

  const data = inbox.data;
  const items = data?.items ?? [];
  const shown = kinds.length > 0 ? items.filter((item) => kinds.includes(item.kind)) : items;
  const filtering = boardId !== null || kinds.length > 0;

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

  function move(from: number, to: number) {
    const next = moveItem(items, from, to);
    if (next !== items) save.mutate(next);
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
        <button
          type="button"
          className="btn"
          disabled={reset.isPending}
          onClick={() => {
            reset.mutate();
          }}
        >
          Reset order
        </button>
      </div>
      {filtering && shown.length > 1 && (
        <p className="muted inbox-hint">Reordering works only without filters.</p>
      )}
      {save.error && <FormError message={`Could not save the order: ${save.error.message}`} />}
      {reset.error && <FormError message={reset.error.message} />}
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
        <ol className="inbox-list" aria-label="Entries">
          {shown.map((item, index) => (
            <InboxRow
              key={`${item.type}:${item.key}`}
              item={item}
              index={index}
              draggable={!filtering}
              onDragStart={(at) => {
                dragFrom.current = at;
              }}
              onDrop={(at) => {
                if (dragFrom.current !== null) move(dragFrom.current, at);
                dragFrom.current = null;
              }}
            />
          ))}
        </ol>
      )}
    </div>
  );
}
