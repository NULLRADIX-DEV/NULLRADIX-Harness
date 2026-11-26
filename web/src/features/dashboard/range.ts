import { addDays, isoDay } from './format';

export type StatsKind = 'work' | 'merge' | 'research';
export type Period = '7' | '28' | '56' | 'config' | 'custom';

export const KINDS: readonly { value: StatsKind; label: string }[] = [
  { value: 'work', label: 'Work' },
  { value: 'research', label: 'Research' },
  { value: 'merge', label: 'Merge' },
];

export const PERIODS: readonly { value: Period; label: string }[] = [
  { value: '7', label: 'Last 7 days' },
  { value: '28', label: 'Last 4 weeks' },
  { value: '56', label: 'Last 8 weeks' },
  { value: 'config', label: 'Since config change' },
  { value: 'custom', label: 'Custom...' },
];

export const DEFAULT_PERIOD: Period = '56';
const DAY = /^\d{4}-\d{2}-\d{2}$/;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
// A custom range spans at most this many days before its last day.
export const MAX_SPAN_DAYS = 365;

// The filter the address holds; from and to only with the custom period.
export interface DashboardFilter {
  boardId: string | null;
  kind: StatsKind;
  period: Period;
  from?: string;
  to?: string;
}

// A range with `to` no later than today and `from` at most a year before `to`.
export function clampRange(
  range: { from: string; to: string },
  today: string,
): { from: string; to: string } {
  const to = range.to < today ? range.to : today;
  const earliest = addDays(to, -MAX_SPAN_DAYS);
  const from = range.from > to ? to : range.from < earliest ? earliest : range.from;
  return { from, to };
}

// Unknown values, and boards not among the known ones, fall back to the defaults.
export function parseFilter(
  search: URLSearchParams,
  today: Date,
  boardIds?: readonly string[],
): DashboardFilter {
  const kind = search.get('kind');
  const period = search.get('period');
  const from = search.get('from') ?? '';
  const to = search.get('to') ?? '';
  const board = search.get('board') ?? '';
  const custom = period === 'custom' && DAY.test(from) && DAY.test(to);
  const preset = PERIODS.find((p) => p.value === period && p.value !== 'custom')?.value;
  const ordered = from <= to ? { from, to } : { from: to, to: from };
  const known = UUID.test(board) && (boardIds === undefined || boardIds.includes(board));
  return {
    boardId: known ? board : null,
    kind: KINDS.find((k) => k.value === kind)?.value ?? 'work',
    period: custom ? 'custom' : (preset ?? DEFAULT_PERIOD),
    ...(custom ? clampRange(ordered, isoDay(today)) : {}),
  };
}

// The address parameters of a filter; defaults stay out.
export function filterParams(filter: DashboardFilter): URLSearchParams {
  const params = new URLSearchParams();
  if (filter.boardId) params.set('board', filter.boardId);
  if (filter.kind !== 'work') params.set('kind', filter.kind);
  if (filter.period !== DEFAULT_PERIOD) params.set('period', filter.period);
  if (filter.period === 'custom' && filter.from && filter.to) {
    params.set('from', filter.from);
    params.set('to', filter.to);
  }
  return params;
}

export interface StatsParams {
  boardId: string | null;
  kind: StatsKind;
  from: string;
  // left out, the daemon counts up to its own today
  to?: string;
}

// The first day of the last n days up to and including today.
export function startOf(today: Date, days: number): string {
  return addDays(isoDay(today), -(days - 1));
}

export function statsParams(filter: DashboardFilter, today: Date): StatsParams {
  const base = { boardId: filter.boardId, kind: filter.kind };
  if (filter.period === 'custom' && filter.from && filter.to) {
    return { ...base, from: filter.from, to: filter.to };
  }
  if (filter.period === 'config') return { ...base, from: 'config' };
  return { ...base, from: startOf(today, Number(filter.period)) };
}

// Moving one edge past the other pulls the other edge along; the range stays within the clamp.
export function withEdge(
  range: { from: string; to: string },
  edge: 'from' | 'to',
  day: string,
  today: string,
): { from: string; to: string } {
  if (!DAY.test(day)) return range;
  const next = { ...range, [edge]: day };
  return clampRange(next.from <= next.to ? next : { from: day, to: day }, today);
}
