export interface EdgeWindow {
  from: string;
  to: string;
}

const DAY_MS = 24 * 60 * 60 * 1000;

function minuteOfDay(time: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(time);
  if (!match) return null;
  return Number(match[1]) * 60 + Number(match[2]);
}

function untilNext(now: Date, minute: number): number {
  const target = new Date(now);
  target.setHours(0, minute, 0, 0);
  const wait = target.getTime() - now.getTime();
  return wait > 0 ? wait : wait + DAY_MS;
}

// Milliseconds until the pause ends or the window opens or closes, whichever is first.
export function nextEdgeDelay(
  now: Date,
  pausedUntil: number | null | undefined,
  window: EdgeWindow | null | undefined,
): number | null {
  const delays: number[] = [];
  if (pausedUntil != null && pausedUntil > now.getTime()) delays.push(pausedUntil - now.getTime());
  if (window) {
    for (const edge of [window.from, window.to]) {
      const minute = minuteOfDay(edge);
      if (minute !== null) delays.push(untilNext(now, minute));
    }
  }
  return delays.length > 0 ? Math.min(...delays) : null;
}
