import type { AbortCategory, Dispatch, Run, RunKind, RunStatus, RunUsage } from './queries';

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

export type RunTone = 'running' | 'ok' | 'warn' | 'error' | 'neutral';

export const ABORT_LABELS: Readonly<Record<AbortCategory, string>> = {
  timeout: 'Timeout',
  'max-turns': 'Turn limit',
  context: 'Context full',
  'exit-code': 'Exit code',
  'claude-error': 'Claude error',
  'usage-limit': 'Usage limit',
  'start-failed': 'Start failed',
  vanished: 'Vanished',
};

export const KIND_TITLES: Readonly<Record<RunKind, string>> = {
  work: 'Work run',
  merge: 'Merge session',
  research: 'Research',
  plan: 'Planning',
};

export interface RunOutcome {
  label: string;
  tone: RunTone;
}

// The badge of a run: its state, and for a failure why it was aborted.
export function runOutcome(run: Run): RunOutcome {
  switch (run.status) {
    case 'starting':
    case 'running':
      return { label: 'Running', tone: 'running' };
    case 'succeeded':
      return { label: 'Succeeded', tone: 'ok' };
    case 'failed': {
      const category = run.result?.abort?.category;
      return { label: category ? `Failed: ${ABORT_LABELS[category]}` : 'Failed', tone: 'error' };
    }
    case 'cancelled':
      return { label: 'Cancelled', tone: 'warn' };
    case 'timed-out':
      return { label: 'Timed out', tone: 'error' };
    case 'lost':
      return { label: 'Lost', tone: 'error' };
  }
}

export function runTitle(run: Run): string {
  const kind = run.dispatch?.kind;
  if (!kind) return run.label;
  return kind === 'work' ? `${KIND_TITLES[kind]} of ${formatStart(run)}` : KIND_TITLES[kind];
}

export function formatCost(usd: number): string {
  const digits = usd !== 0 && Math.abs(usd) < 0.01 ? 3 : 2;
  return `$${usd.toFixed(digits)}`;
}

function oneDecimal(value: number): string {
  return value.toFixed(1).replace(/\.0$/, '');
}

// 950 stays, 12 400 becomes 12.4k, 1 500 000 becomes 1.5M.
export function formatTokens(count: number): string {
  if (count < 1000) return String(count);
  if (count < 1_000_000) return `${oneDecimal(count / 1000)}k`;
  return `${oneDecimal(count / 1_000_000)}M`;
}

export function shortModel(model: string | null | undefined): string | null {
  return model ? model.replace(/^claude-/, '') : null;
}

export function runModel(run: Run): string | null {
  return shortModel(run.result?.usage?.model ?? run.dispatch?.profile.model);
}

// Settings a dispatched run started with, as short chips.
export function runChips(dispatch: Dispatch | null | undefined): string[] {
  if (!dispatch) return [];
  const { profile } = dispatch;
  const chips = [
    `Profile ${profile.name}`,
    shortModel(profile.model),
    profile.effort ? `Effort ${profile.effort}` : null,
    `${String(profile.maxTurns)} turns max`,
    `${String(profile.timeoutMinutes)} min timeout`,
    dispatch.trigger === 'manual' ? 'Started by hand' : 'Started by the dispatcher',
  ];
  return chips.filter((chip): chip is string => chip !== null);
}

type UsageFigures = Partial<Omit<RunUsage, 'model' | 'models'>>;

// Token counts and turns of one usage record, such as "1.2k in, 340 out".
export function usageLine(usage: UsageFigures): string {
  const parts: [number | null | undefined, string][] = [
    [usage.inputTokens, 'in'],
    [usage.outputTokens, 'out'],
    [usage.cacheReadTokens, 'cache read'],
    [usage.cacheWriteTokens, 'cache write'],
  ];
  const tokens = parts.flatMap(([count, label]) =>
    count == null ? [] : [`${formatTokens(count)} ${label}`],
  );
  if (usage.numTurns != null) tokens.push(`${String(usage.numTurns)} turns`);
  if (usage.costUsd != null) tokens.push(formatCost(usage.costUsd));
  return tokens.join(', ');
}

// Input and output tokens together, when Claude reported any.
export function totalTokens(usage: RunUsage | null | undefined): number | null {
  if (!usage) return null;
  if (usage.inputTokens == null && usage.outputTokens == null) return null;
  return (usage.inputTokens ?? 0) + (usage.outputTokens ?? 0);
}
