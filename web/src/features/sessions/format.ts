import { useMemo } from 'react';
import { useNow } from '../../shared/lib/useNow';
import type { DisplayState, Session } from './queries';

export const STATE_LABELS: Readonly<Record<DisplayState, string>> = {
  running: 'Running',
  waiting: 'Waiting for input',
  idle: 'Idle',
  ended: 'Ended',
  stale: 'Stale',
};

// Filter order of the states; active ones first.
export const STATES: readonly DisplayState[] = ['running', 'waiting', 'idle', 'stale', 'ended'];

// What the page shows at first, as in legacy: running and idle.
export const DEFAULT_STATES: ReadonlySet<DisplayState> = new Set<DisplayState>(['running', 'idle']);

const STATE_ORDER: Readonly<Record<DisplayState, number>> = {
  running: 0,
  waiting: 1,
  idle: 2,
  stale: 3,
  ended: 4,
};

// Minutes, hours with minutes, or days with hours: "5 min", "1 h 05 min", "2 d 3 h".
export function formatDuration(millis: number): string {
  const minutes = Math.floor(millis / 60_000);
  if (minutes < 1) return 'less than 1 min';
  if (minutes < 60) return `${String(minutes)} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${String(hours)} h ${String(minutes % 60).padStart(2, '0')} min`;
  return `${String(Math.floor(hours / 24))} d ${String(hours % 24)} h`;
}

// Time since `at`: "just now", "5 min ago", "2 h ago", "3 d ago".
export function formatAgo(at: number, now: number): string {
  const minutes = Math.floor((now - at) / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${String(minutes)} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${String(hours)} h ago`;
  return `${String(Math.floor(hours / 24))} d ago`;
}

// Active states first, within a state the latest activity first.
export function bySessionOrder(a: Session, b: Session): number {
  const byState = STATE_ORDER[a.displayState] - STATE_ORDER[b.displayState];
  return byState === 0 ? b.lastEventAt - a.lastEventAt : byState;
}

export interface SessionGroup {
  ticket: { key: string; title: string } | null;
  sessions: Session[];
}

// One group per ticket by key; a session shows once, at its lowest ticket; loose ones come last.
export function groupByTicket(sessions: readonly Session[]): SessionGroup[] {
  const byTicket = new Map<string, SessionGroup>();
  const loose: Session[] = [];
  for (const session of [...sessions].sort(bySessionOrder)) {
    const [first] = [...session.tickets].sort((a, b) => a.key.localeCompare(b.key));
    if (!first) {
      loose.push(session);
      continue;
    }
    const group = byTicket.get(first.key) ?? {
      ticket: { key: first.key, title: first.title },
      sessions: [],
    };
    group.sessions.push(session);
    byTicket.set(first.key, group);
  }
  const groups = [...byTicket.values()].sort((a, b) =>
    (a.ticket?.key ?? '').localeCompare(b.ticket?.key ?? ''),
  );
  return loose.length > 0 ? [...groups, { ticket: null, sessions: loose }] : groups;
}

// Counts of the states that need attention: "1 running, 2 stale".
export function summarize(sessions: readonly Session[]): string {
  return (['running', 'waiting', 'stale'] as const)
    .map((state) => [state, sessions.filter((each) => each.displayState === state).length] as const)
    .filter(([, count]) => count > 0)
    .map(([state, count]) => `${String(count)} ${STATE_LABELS[state].toLowerCase()}`)
    .join(', ');
}

// The name to show; legacy fell back to "Untitled".
export function sessionTitle(session: Session): string {
  return session.label === '' ? 'Untitled' : session.label;
}

// Runtime until the end, or until now while the session lasts.
export function sessionRuntime(session: Session, now: number): number {
  return Math.max(0, (session.endedAt ?? now) - session.startedAt);
}

// Without a hook for this long a live session shows as stale, as the daemon decides.
export const STALE_MS = 30 * 60_000;

// The state as of `now`: the daemon computed it when read, so a page kept open recomputes it.
export function withLiveState(session: Session, now: number): Session {
  if (session.state === 'ended') return session;
  const stale = now - session.lastEventAt > STALE_MS;
  return {
    ...session,
    displayState: stale ? 'stale' : session.state,
    listening: !stale,
  };
}

// Sessions as of `now`; the clock ticks while any are shown, for states and "ago" texts.
export function useLiveSessions(reported: readonly Session[] | undefined): {
  sessions: Session[];
  now: number;
} {
  const now = useNow((reported ?? []).length > 0);
  const sessions = useMemo(
    () => (reported ?? []).map((session) => withLiveState(session, now)),
    [reported, now],
  );
  return { sessions, now };
}
