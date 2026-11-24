import { useQuery } from '@tanstack/react-query';
import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { FormError } from '../../shared/ui/FormError';
import { ticketQuery, type Ticket, type TicketDetail, type TicketStatus } from '../tickets/queries';
import { AcceptanceDialog } from './AcceptanceDialog';
import { loadProblem, ticketProblem } from './messages';
import { TicketDependencies } from './TicketDependencies';
import { TicketProperties } from './TicketProperties';
import { TicketTabs } from './TicketTabs';
import { useDraft } from './useDraft';
import { useTicketActions, type TicketActions } from './useTicketActions';

function oneLine(text: string): string {
  return text.replace(/[ \t]*\r?\n[ \t]*/g, ' ');
}

// The title grows with its text; Enter or leaving the field saves it.
function TitleField({ ticket, actions }: { ticket: Ticket; actions: TicketActions }) {
  const [draft, setDraft] = useDraft(ticket.title);

  function commit() {
    if (actions.busy || draft.trim() === '' || draft === ticket.title) return;
    void actions.save({ title: draft });
  }

  return (
    <textarea
      className="db-title"
      aria-label="Title"
      rows={1}
      value={draft}
      onChange={(event) => {
        setDraft(oneLine(event.target.value));
      }}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key !== 'Enter') return;
        event.preventDefault();
        commit();
      }}
    />
  );
}

function PanelHead({
  ticketKey,
  meta,
  onClose,
  children,
}: {
  ticketKey: string;
  meta?: ReactNode;
  onClose: () => void;
  children?: ReactNode;
}) {
  return (
    <div className="db-panel-head">
      <button
        type="button"
        className="icon-btn db-panel-close"
        aria-label="Close"
        title="Close"
        onClick={onClose}
      >
        &times;
      </button>
      <div className="db-panel-heading">
        <div className="db-panel-meta">
          <span className="card-id">{ticketKey}</span>
          {meta && <span className="muted">{meta}</span>}
        </div>
        {children}
      </div>
    </div>
  );
}

function DeleteTicket({
  ticket,
  actions,
  onDeleted,
}: {
  ticket: Ticket;
  actions: TicketActions;
  onDeleted: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  if (!confirming) {
    return (
      <div className="db-panel-foot">
        <button
          type="button"
          className="btn danger"
          onClick={() => {
            setConfirming(true);
          }}
        >
          Delete ticket
        </button>
      </div>
    );
  }
  return (
    <div className="db-panel-foot">
      <div
        role="alertdialog"
        aria-label={`Delete ${ticket.key}`}
        className="actions"
        onKeyDown={(event) => {
          if (event.key !== 'Escape') return;
          event.stopPropagation();
          setConfirming(false);
        }}
      >
        <span>Delete {ticket.key} for good? This cannot be undone.</span>
        <button
          type="button"
          className="btn danger"
          disabled={actions.busy}
          onClick={() => {
            void actions.remove().then((removed) => {
              if (removed) onDeleted();
            });
          }}
        >
          Delete for good
        </button>
        <button
          type="button"
          className="btn"
          autoFocus
          onClick={() => {
            setConfirming(false);
          }}
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

function TicketView({
  detail,
  loadError,
  onClose,
  onOpen,
}: {
  detail: TicketDetail;
  loadError: string | null;
  onClose: () => void;
  onOpen: (key: string) => void;
}) {
  const { ticket } = detail;
  const actions = useTicketActions(ticket);
  const [askAcceptance, setAskAcceptance] = useState(false);

  function changeStatus(status: TicketStatus) {
    if (status === ticket.status) return;
    if (status === 'done' && ticket.acceptance.trim() === '') setAskAcceptance(true);
    else void actions.move(status);
  }

  return (
    <>
      <PanelHead
        ticketKey={ticket.key}
        meta={
          <>
            Version {ticket.version} &middot; created by {ticket.createdByName}
          </>
        }
        onClose={onClose}
      >
        <TitleField ticket={ticket} actions={actions} />
      </PanelHead>
      <div className="db-panel-body">
        <div className="db-panel-main">
          <FormError message={loadError ?? ticketProblem(actions.failure, ticket)} />
          <TicketProperties ticket={ticket} actions={actions} onStatus={changeStatus} />
          <TicketTabs detail={detail} actions={actions} />
        </div>
        <div className="db-panel-side">
          <TicketDependencies ticket={ticket} onOpen={onOpen} />
          <DeleteTicket ticket={ticket} actions={actions} onDeleted={onClose} />
        </div>
      </div>
      {askAcceptance && (
        <AcceptanceDialog
          ticketKey={ticket.key}
          onCancel={() => {
            setAskAcceptance(false);
          }}
          onConfirm={(acceptance) => {
            setAskAcceptance(false);
            void actions.move('done', acceptance);
          }}
        />
      )}
    </>
  );
}

// Side panel of one ticket over the board; Escape closes it and focus returns to the card.
export function TicketPanel({
  ticketKey,
  onClose,
  onOpen,
}: {
  ticketKey: string;
  onClose: () => void;
  onOpen: (key: string) => void;
}) {
  const { status, data, error } = useQuery(ticketQuery(ticketKey));
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    const node = ref.current;
    const previous = document.activeElement;
    if (node && !node.contains(previous)) node.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, []);

  function onKeyDown(event: KeyboardEvent<HTMLElement>) {
    if (event.key === 'Escape') onClose();
  }

  return (
    <aside
      ref={ref}
      tabIndex={-1}
      className="db-panel"
      role="dialog"
      aria-modal="false"
      aria-label={`Ticket ${ticketKey}`}
      onKeyDown={onKeyDown}
    >
      {data ? (
        <TicketView
          detail={data}
          loadError={status === 'error' ? loadProblem(error) : null}
          onClose={onClose}
          onOpen={onOpen}
        />
      ) : (
        <>
          <PanelHead ticketKey={ticketKey} onClose={onClose} />
          <div className="db-panel-state">
            {status === 'pending' ? (
              <p className="muted">Loading ticket...</p>
            ) : (
              <FormError message={loadProblem(error)} />
            )}
          </div>
        </>
      )}
    </aside>
  );
}
