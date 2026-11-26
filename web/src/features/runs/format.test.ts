import { describe, expect, it } from 'vitest';
import { dispatchRun } from '../../test/runFixtures';
import {
  formatCost,
  formatDuration,
  formatTokens,
  runChips,
  runDuration,
  runModel,
  runOutcome,
  runTitle,
  totalTokens,
  usageLine,
} from './format';
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

describe('formatCost', () => {
  it('shows cents and keeps tiny costs visible', () => {
    expect(formatCost(0.428)).toBe('$0.43');
    expect(formatCost(0)).toBe('$0.00');
    expect(formatCost(0.004)).toBe('$0.004');
    expect(formatCost(12)).toBe('$12.00');
  });
});

describe('formatTokens', () => {
  it('shortens thousands and millions', () => {
    expect(formatTokens(950)).toBe('950');
    expect(formatTokens(12_400)).toBe('12.4k');
    expect(formatTokens(2_000)).toBe('2k');
    expect(formatTokens(1_500_000)).toBe('1.5M');
  });
});

describe('runOutcome', () => {
  it('names the state and, for a failure, the abort category', () => {
    expect(runOutcome(base)).toEqual({ label: 'Running', tone: 'running' });
    expect(runOutcome({ ...base, status: 'succeeded' })).toEqual({
      label: 'Succeeded',
      tone: 'ok',
    });
    expect(runOutcome({ ...base, status: 'failed' })).toEqual({ label: 'Failed', tone: 'error' });
    const aborted = {
      ...base,
      status: 'failed' as const,
      result: { abort: { category: 'max-turns' as const, reason: 'x' } },
    };
    expect(runOutcome(aborted)).toEqual({ label: 'Failed: Turn limit', tone: 'error' });
    expect(runOutcome({ ...base, status: 'cancelled' }).tone).toBe('warn');
    expect(runOutcome({ ...base, status: 'timed-out' }).label).toBe('Timed out');
    expect(runOutcome({ ...base, status: 'lost' }).label).toBe('Lost');
  });
});

describe('runTitle', () => {
  it('names the kind of a dispatched run and falls back to the label', () => {
    expect(runTitle(base)).toBe('build');
    expect(runTitle(dispatchRun())).toMatch(/^Work run of /);
    const merge = dispatchRun();
    if (merge.dispatch) merge.dispatch.kind = 'merge';
    expect(runTitle(merge)).toBe('Merge session');
    if (merge.dispatch) merge.dispatch.kind = 'research';
    expect(runTitle(merge)).toBe('Research');
    if (merge.dispatch) merge.dispatch.kind = 'plan';
    expect(runTitle(merge)).toBe('Planning');
  });
});

describe('runModel and totalTokens', () => {
  it('strips the claude prefix and sums input and output', () => {
    expect(runModel(dispatchRun())).toBe('sonnet-5');
    expect(runModel(base)).toBeNull();
    expect(totalTokens(dispatchRun().result?.usage)).toBe(15_500);
    expect(totalTokens(null)).toBeNull();
    expect(totalTokens({ models: [] })).toBeNull();
  });
});

describe('runChips and usageLine', () => {
  it('lists the settings of a dispatched run', () => {
    expect(runChips(undefined)).toEqual([]);
    expect(runChips(dispatchRun().dispatch)).toEqual([
      'Profile standard',
      'sonnet-5',
      'Effort high',
      '40 turns max',
      '30 min timeout',
      'Started by the dispatcher',
    ]);
  });

  it('sums up the figures of a usage record', () => {
    const usage = dispatchRun().result?.usage;
    expect(usage && usageLine(usage)).toBe(
      '12.4k in, 3.1k out, 80k cache read, 900 cache write, 12 turns, $0.43',
    );
    expect(usageLine({ inputTokens: 5 })).toBe('5 in');
  });
});
