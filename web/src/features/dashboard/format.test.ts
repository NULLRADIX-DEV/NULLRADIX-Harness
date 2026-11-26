import { describe, expect, it } from 'vitest';
import {
  NONE,
  addDays,
  formatCost,
  formatCount,
  formatDay,
  formatDuration,
  formatPercent,
  isoDay,
  plural,
  waitText,
} from './format';

describe('formatPercent', () => {
  it('shows fractions as whole percent', () => {
    expect(formatPercent(0)).toBe('0 %');
    expect(formatPercent(1)).toBe('100 %');
    expect(formatPercent(0.625)).toBe('63 %');
    expect(formatPercent(0.624)).toBe('62 %');
    expect(formatPercent(0.3333)).toBe('33 %');
  });

  it('rounds like the legacy dashboard, with its float noise', () => {
    expect(formatPercent(0.285)).toBe('28 %');
    expect(formatPercent(0.005)).toBe('1 %');
  });

  it('shows a dash for a missing rate', () => {
    expect(formatPercent(null)).toBe(NONE);
    expect(formatPercent(undefined)).toBe(NONE);
  });
});

describe('other figures', () => {
  it('shortens durations', () => {
    expect(formatDuration(null)).toBe(NONE);
    expect(formatDuration(45 * 60_000)).toBe('45 min');
    expect(formatDuration(3.5 * 3_600_000)).toBe('3.5 h');
    expect(formatDuration(50 * 3_600_000)).toBe('2.1 days');
  });

  it('writes costs with two decimals, three below a cent', () => {
    expect(formatCost(null)).toBe(NONE);
    expect(formatCost(0)).toBe('$0.00');
    expect(formatCost(12.345)).toBe('$12.35');
    expect(formatCost(0.004)).toBe('$0.004');
    expect(formatCost(1234.5)).toBe('$1,234.50');
  });

  it('shortens counts', () => {
    expect(formatCount(950)).toBe('950');
    expect(formatCount(12_300)).toBe('12.3k');
    expect(formatCount(4_100_000)).toBe('4.1M');
    expect(formatCount(null)).toBe(NONE);
  });

  it('picks the singular or plural', () => {
    expect(plural(1, 'run', 'runs')).toBe('1 run');
    expect(plural(0, 'run', 'runs')).toBe('0 runs');
    expect(plural(3, 'run', 'runs')).toBe('3 runs');
  });

  it('formats days and counts them', () => {
    expect(formatDay('2026-11-04')).toBe('4 Nov');
    expect(formatDay('2026-11-04', true)).toBe('4 Nov 2026');
    expect(isoDay(new Date(2026, 0, 5))).toBe('2026-01-05');
    expect(addDays('2026-11-30', 1)).toBe('2026-12-01');
    expect(addDays('2026-03-01', -1)).toBe('2026-02-28');
  });

  it('writes waiting times short', () => {
    const now = 100 * 3_600_000;
    expect(waitText(now - 40 * 60_000, now)).toBe('40 min');
    expect(waitText(now - 5 * 3_600_000, now)).toBe('5 h');
    expect(waitText(now - 24 * 3_600_000, now)).toBe('1 day');
    expect(waitText(now - 72 * 3_600_000, now)).toBe('3 days');
  });
});
