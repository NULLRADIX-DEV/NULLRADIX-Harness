import { describe, expect, it } from 'vitest';
import { nextEdgeDelay } from './statusEdge';

const now = new Date(2027, 0, 15, 8, 0, 30);
const HOUR = 3_600_000;

describe('nextEdgeDelay', () => {
  it('has no edge without a pause and a window', () => {
    expect(nextEdgeDelay(now, null, null)).toBeNull();
    expect(nextEdgeDelay(now, undefined, undefined)).toBeNull();
    expect(nextEdgeDelay(now, now.getTime() - 1, null)).toBeNull();
  });

  it('waits for the pause to end', () => {
    expect(nextEdgeDelay(now, now.getTime() + 90_000, null)).toBe(90_000);
  });

  it('waits for the window to open, then to close', () => {
    const window = { from: '13:00', to: '14:00' };
    expect(nextEdgeDelay(now, null, window)).toBe(5 * HOUR - 30_000);
    const inside = new Date(2027, 0, 15, 13, 30, 0);
    expect(nextEdgeDelay(inside, null, window)).toBe(HOUR / 2);
  });

  it('wraps past midnight and picks the nearest edge', () => {
    const window = { from: '22:00', to: '06:00' };
    const late = new Date(2027, 0, 15, 23, 0, 0);
    expect(nextEdgeDelay(late, null, window)).toBe(7 * HOUR);
    expect(nextEdgeDelay(late, late.getTime() + 60_000, window)).toBe(60_000);
  });
});
