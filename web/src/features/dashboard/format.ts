// Numbers of the dashboard as text; a missing value shows as a dash.
export const NONE = '-';

// 0..1 as whole percent, rounded half up like the legacy dashboard.
export function formatPercent(rate: number | null | undefined): string {
  return rate === null || rate === undefined ? NONE : `${String(Math.round(rate * 100))} %`;
}

// One decimal place, none when it would be zero.
export function decimal(n: number): string {
  return n.toLocaleString('en-US', { maximumFractionDigits: 1 });
}

// "45 min", "3.5 h", "2.1 days"
export function formatDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return NONE;
  const minutes = ms / 60_000;
  if (minutes < 90) return `${String(Math.round(minutes))} min`;
  const hours = minutes / 60;
  if (hours < 36) return `${decimal(hours)} h`;
  return `${decimal(hours / 24)} days`;
}

// US dollars with two decimals, three below one cent.
export function formatCost(usd: number | null | undefined): string {
  if (usd === null || usd === undefined) return NONE;
  const digits = usd !== 0 && Math.abs(usd) < 0.01 ? 3 : 2;
  return `$${usd.toLocaleString('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits })}`;
}

// "950", "12.3k", "4.1M"
export function formatCount(n: number | null | undefined): string {
  if (n === null || n === undefined) return NONE;
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${decimal(n / 1000)}k`;
  return `${decimal(n / 1_000_000)}M`;
}

export function plural(n: number, one: string, many: string): string {
  return `${String(n)} ${n === 1 ? one : many}`;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

// `YYYY-MM-DD` as "24 Nov", or "24 Nov 2026" with the year.
export function formatDay(day: string, withYear = false): string {
  const [year, month, date] = day.slice(0, 10).split('-');
  const name = MONTHS[Number(month) - 1] ?? '';
  const text = `${String(Number(date))} ${name}`;
  return withYear ? `${text} ${year ?? ''}` : text;
}

// A day in local time as `YYYY-MM-DD`.
export function isoDay(date: Date): string {
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${String(date.getFullYear())}-${month}-${day}`;
}

// The day `days` after (or before) a `YYYY-MM-DD` day; local noon keeps clock shifts out.
export function addDays(day: string, days: number): string {
  const [y, m, d] = day.split('-').map(Number);
  return isoDay(new Date(y ?? 1970, (m ?? 1) - 1, (d ?? 1) + days, 12));
}

// Waiting time short: "40 min", "5 h", "3 days".
export function waitText(sinceMs: number, nowMs: number): string {
  const hours = Math.max(0, nowMs - sinceMs) / 3_600_000;
  if (hours < 1) return `${String(Math.round(hours * 60))} min`;
  if (hours < 24) return `${String(Math.round(hours))} h`;
  const days = Math.round(hours / 24);
  return `${String(days)} ${days === 1 ? 'day' : 'days'}`;
}
