import { describe, expect, it } from 'vitest';
import { formatDuration, runDuration } from './format';
import type { Run } from './queries';

const base: Run = {
  id: '0190a0b0-0000-7000-8000-0000000000a1',
  label: 'build',
  status: 'running',
  createdAt: 0,
  version: 1,
};

describe('formatDuration', () => {
  it('grows from seconds to minutes to hours', () => {
    expect(formatDuration(-5)).toBe('0 s');
    expect(formatDuration(59_400)).toBe('59 s');
    expect(formatDuration(61_000)).toBe('1 min');
    expect(formatDuration(3_600_000)).toBe('1 h');
    expect(formatDuration(3_900_000)).toBe('1 h 5 min');
  });
});

describe('runDuration', () => {
  it('counts to the end, or to now while running, and is unknown before the start', () => {
    expect(runDuration(base, 10_000)).toBeNull();
    expect(runDuration({ ...base, startedAt: 1_000 }, 11_000)).toBe('10 s');
    expect(runDuration({ ...base, startedAt: 1_000, finishedAt: 4_000 }, 99_000)).toBe('3 s');
  });
});
