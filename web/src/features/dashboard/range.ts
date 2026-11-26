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

// The filter the address holds; from and to only with the custom period.
export interface DashboardFilter {
  boardId: string | null;
  kind: StatsKind;
  period: Period;
  from?: string;
  to?: string;
}

// Unknown values fall back to the defaults; a custom range without two valid days to 56 days.
export function parseFilter(search: URLSearchParams): DashboardFilter {
  const kind = search.get('kind');
  const period = search.get('period');
  const from = search.get('from') ?? '';
  const to = search.get('to') ?? '';
  const custom = period === 'custom' && DAY.test(from) && DAY.test(to);
  const preset = PERIODS.find((p) => p.value === period && p.value !== 'custom')?.value;
  return {
    boardId: search.get('board') === '' ? null : search.get('board'),
    kind: KINDS.find((k) => k.value === kind)?.value ?? 'work',
    period: custom ? 'custom' : (preset ?? DEFAULT_PERIOD),
    ...(custom ? (from <= to ? { from, to } : { from: to, to: from }) : {}),
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
  to: string;
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
  const to = isoDay(today);
  if (filter.period === 'config') return { ...base, from: 'config', to };
  return { ...base, from: startOf(today, Number(filter.period)), to };
}

// Moving one edge past the other pulls the other edge along.
export function withEdge(
  range: { from: string; to: string },
  edge: 'from' | 'to',
  day: string,
): { from: string; to: string } {
  if (!DAY.test(day)) return range;
  const next = { ...range, [edge]: day };
  return next.from <= next.to ? next : { from: day, to: day };
}
