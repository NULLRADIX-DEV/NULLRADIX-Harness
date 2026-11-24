import type { Column, Priority, Ticket, TicketStatus } from './queries';

export const COLUMNS: readonly Column[] = [
  'backlog',
  'waiting',
  'in-planning',
  'planned',
  'in-progress',
  'blocked',
  'done',
  'not-needed',
];

export const STATUSES: readonly TicketStatus[] = [
  'backlog',
  'in-planning',
  'planned',
  'in-progress',
  'blocked',
  'done',
  'not-needed',
];

export const PRIORITIES: readonly Priority[] = ['urgent', 'high', 'normal', 'low'];

export const COLUMN_LABELS: Readonly<Record<Column, string>> = {
  backlog: 'Backlog',
  waiting: 'Waiting',
  'in-planning': 'In planning',
  planned: 'Planned',
  'in-progress': 'In progress',
  blocked: 'Blocked',
  done: 'Done',
  'not-needed': 'Not needed',
};

export const PRIORITY_LABELS: Readonly<Record<Priority, string>> = {
  urgent: 'Urgent',
  high: 'High',
  normal: 'Normal',
  low: 'Low',
};

export function isStatus(value: string): value is TicketStatus {
  return (STATUSES as readonly string[]).includes(value);
}

export function isPriority(value: string): value is Priority {
  return (PRIORITIES as readonly string[]).includes(value);
}

export function splitList(text: string): string[] {
  return text
    .split(',')
    .map((item) => item.trim())
    .filter((item) => item !== '');
}

// Tickets per column, each in the order the daemon returned them.
export function byColumn(tickets: readonly Ticket[]): ReadonlyMap<Column, Ticket[]> {
  const columns = new Map<Column, Ticket[]>(COLUMNS.map((column) => [column, []]));
  for (const ticket of tickets) columns.get(ticket.column)?.push(ticket);
  return columns;
}
