import { COLUMN_LABELS, PRIORITY_LABELS, isPriority } from '../tickets/labels';
import type { HistoryLine } from '../tickets/queries';

// Color of the dot: success, failure, a person acting, waiting, neutral.
export type Tone = 'ok' | 'error' | 'info' | 'warn' | 'plain';

export interface HistoryView {
  tone: Tone;
  title: string;
  meta: string[];
  // Done, blocked and similar milestones stand out.
  highlight: boolean;
  error?: string;
  detail?: string;
}

export const FIELD_LABELS: Readonly<Partial<Record<string, string>>> = {
  status: 'Status',
  title: 'Title',
  priority: 'Priority',
  assignee: 'Assignee',
  labels: 'Labels',
  dependsOn: 'Depends on',
  needsPlanning: 'Needs planning',
  blockedReason: 'Blocked reason',
  acceptance: 'Acceptance',
  description: 'Description',
  plan: 'Plan',
  handbook: 'Handbook',
  diagram: 'Diagram',
  boardId: 'Board',
  plannedBy: 'Planned by',
  planningClaimBy: 'Planning claim',
  targetBranch: 'Target branch',
  step: 'Step',
};

const LONG_TEXTS = new Set(['description', 'plan', 'handbook', 'diagram']);
const LISTS = new Set(['labels', 'dependsOn']);
const EMPTY_TEXT = '0 characters';
const NONE = String.fromCharCode(0x2013);
const ARROW = String.fromCharCode(0x2192);

function statusLabel(value: string | null | undefined): string {
  if (!value) return NONE;
  return (COLUMN_LABELS as Readonly<Partial<Record<string, string>>>)[value] ?? value;
}

function priorityLabel(value: string | null | undefined): string {
  if (!value) return NONE;
  return isPriority(value) ? PRIORITY_LABELS[value] : value;
}

// A JSON list from the history as comma separated text.
function list(value: string | null | undefined): string {
  if (!value) return NONE;
  try {
    const items: unknown = JSON.parse(value);
    return Array.isArray(items) && items.length > 0 ? items.map(String).join(', ') : NONE;
  } catch {
    return value;
  }
}

function withNote(line: HistoryLine, meta: string[] = [line.actorName]): string[] {
  return line.note ? [...meta, line.note] : meta;
}

// Title, meta line and tone of one history line, as the legacy timeline showed it.
export function historyView(line: HistoryLine): HistoryView {
  const { field, from, to } = line;
  const label = FIELD_LABELS[field] ?? field;
  const base: HistoryView = { tone: 'plain', title: '', meta: withNote(line), highlight: false };
  switch (field) {
    case 'status': {
      const tone: Tone = to === 'done' ? 'ok' : to === 'blocked' ? 'error' : 'plain';
      const title = from
        ? `${statusLabel(from)} ${ARROW} ${statusLabel(to)}`
        : `Created in ${statusLabel(to)}`;
      return { ...base, tone, title, highlight: to === 'done' || to === 'blocked' };
    }
    case 'blockedReason':
      if (!to)
        return { ...base, title: 'Blocked reason removed', ...(from ? { detail: from } : {}) };
      return { ...base, tone: 'error', title: 'Blocked', error: to, highlight: true };
    case 'assignee':
      return { ...base, title: to ? `Taken by ${to}` : `Released (was ${from ?? NONE})` };
    case 'planningClaimBy':
      return { ...base, title: to ? `Planning claimed by ${to}` : 'Planning claim released' };
    case 'priority':
      return { ...base, title: `Priority: ${priorityLabel(from)} ${ARROW} ${priorityLabel(to)}` };
    case 'needsPlanning':
      return { ...base, title: to === 'true' ? 'Needs planning' : 'Planning done' };
    case 'acceptance':
      return { ...base, title: 'Acceptance changed', ...(to ? { detail: to } : {}) };
    case 'step': {
      const meta = line.note && line.note !== to ? [line.actorName, line.note] : [line.actorName];
      return { ...base, title: `Step ${to ?? from ?? ''}`, meta };
    }
  }
  if (LONG_TEXTS.has(field)) {
    const verb = from === EMPTY_TEXT ? 'written' : to === EMPTY_TEXT ? 'cleared' : 'changed';
    return {
      ...base,
      title: `${label} ${verb}`,
      meta: withNote(line, [line.actorName, `${from ?? NONE} ${ARROW} ${to ?? NONE}`]),
    };
  }
  if (LISTS.has(field)) {
    return {
      ...base,
      title: `${label}: ${list(to)}`,
      meta: withNote(line, [line.actorName, `before: ${list(from)}`]),
    };
  }
  return { ...base, title: `${label}: ${from ?? NONE} ${ARROW} ${to ?? NONE}` };
}
