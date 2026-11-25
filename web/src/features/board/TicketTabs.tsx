import { useQuery } from '@tanstack/react-query';
import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { ticketRunsQuery } from '../runs/dispatchQueries';
import { TicketRuns } from '../runs/TicketRuns';
import type { TicketDetail } from '../tickets/queries';
import { TextTab, type TextField } from './TextTab';
import { TicketComments } from './TicketComments';
import { TicketHistory } from './TicketHistory';
import { TicketSteps } from './TicketSteps';
import type { TicketActions } from './useTicketActions';

type TabKey = TextField | 'steps' | 'comments' | 'runs' | 'history';

interface Tab {
  key: TabKey;
  label: string;
  filled: boolean;
  content: ReactNode;
}

const TEXTS: Readonly<Record<TextField, { label: string; empty: string }>> = {
  description: { label: 'Description', empty: 'No description yet.' },
  acceptance: { label: 'Acceptance', empty: 'No acceptance criteria yet.' },
  plan: { label: 'Plan', empty: 'No plan yet.' },
  handbook: { label: 'Handbook', empty: 'No handbook yet.' },
};

// Content of a ticket in tabs; every panel stays mounted so open drafts survive a switch.
export function TicketTabs({ detail, actions }: { detail: TicketDetail; actions: TicketActions }) {
  const { ticket, steps, history, comments } = detail;
  const { data: runs = [] } = useQuery(ticketRunsQuery(ticket.key));
  const [active, setActive] = useState<TabKey>('description');
  const buttons = useRef(new Map<TabKey, HTMLButtonElement>());
  const id = useId();

  const textTab = (field: TextField): Tab => ({
    key: field,
    label: TEXTS[field].label,
    filled: ticket[field].trim() !== '',
    content: (
      <TextTab
        ticket={ticket}
        field={field}
        label={TEXTS[field].label}
        empty={TEXTS[field].empty}
        actions={actions}
      />
    ),
  });
  const tabs: Tab[] = [
    textTab('description'),
    {
      key: 'steps',
      label: `Steps (${String(ticket.stepsDone)}/${String(ticket.stepsTotal)})`,
      filled: false,
      content: <TicketSteps steps={steps} actions={actions} />,
    },
    textTab('acceptance'),
    textTab('plan'),
    textTab('handbook'),
    {
      key: 'comments',
      label: `Comments (${String(comments.length)})`,
      filled: false,
      content: <TicketComments comments={comments} actions={actions} />,
    },
    {
      key: 'runs',
      label: runs.length > 0 ? `Runs (${String(runs.length)})` : 'Runs',
      filled: false,
      content: <TicketRuns ticketKey={ticket.key} />,
    },
    {
      key: 'history',
      label: 'History',
      filled: false,
      content: <TicketHistory history={history} />,
    },
  ];

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();
    const next = tabs[(index + step + tabs.length) % tabs.length];
    if (!next) return;
    setActive(next.key);
    buttons.current.get(next.key)?.focus();
  }

  return (
    <div className="db-tabs">
      <div role="tablist" aria-label="Ticket content" className="db-tablist">
        {tabs.map((tab, index) => (
          <button
            key={tab.key}
            ref={(button) => {
              if (button) buttons.current.set(tab.key, button);
              else buttons.current.delete(tab.key);
            }}
            type="button"
            role="tab"
            id={`${id}-tab-${tab.key}`}
            aria-selected={tab.key === active}
            aria-controls={`${id}-panel-${tab.key}`}
            tabIndex={tab.key === active ? 0 : -1}
            className={tab.key === active ? 'active' : undefined}
            onClick={() => {
              setActive(tab.key);
            }}
            onKeyDown={(event) => {
              onKeyDown(event, index);
            }}
          >
            {tab.label}
            {tab.filled && (
              <span aria-hidden="true" className="db-tab-dot">
                &nbsp;&bull;
              </span>
            )}
          </button>
        ))}
      </div>
      {tabs.map((tab) => (
        <div
          key={tab.key}
          role="tabpanel"
          id={`${id}-panel-${tab.key}`}
          aria-labelledby={`${id}-tab-${tab.key}`}
          className="db-tabpanel"
          hidden={tab.key !== active}
        >
          {tab.content}
        </div>
      ))}
    </div>
  );
}
