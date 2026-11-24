import type { Run, RunStatus } from './queries';

export const STATUS_LABELS: Readonly<Record<RunStatus, string>> = {
  starting: 'Starting',
  running: 'Running',
  succeeded: 'Succeeded',
  failed: 'Failed',
  cancelled: 'Cancelled',
  'timed-out': 'Timed out',
  lost: 'Lost',
};

const TIME = new Intl.DateTimeFormat(undefined, {
  month: 'short',
  day: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
});

export function formatStart(run: Run): string {
  return TIME.format(new Date(run.startedAt ?? run.createdAt));
}

// Whole seconds below a minute, then minutes, then hours with minutes.
export function formatDuration(millis: number): string {
  const seconds = Math.max(0, Math.round(millis / 1000));
  if (seconds < 60) return `${String(seconds)} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${String(minutes)} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${String(hours)} h` : `${String(hours)} h ${String(rest)} min`;
}

// How long the run took, or has taken so far by `now`.
export function runDuration(run: Run, now: number): string | null {
  if (run.startedAt === null || run.startedAt === undefined) return null;
  return formatDuration((run.finishedAt ?? now) - run.startedAt);
}
