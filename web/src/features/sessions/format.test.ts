import { describe, expect, it } from 'vitest';
import { formatAgo, formatDuration, groupByTicket, sessionRuntime, summarize } from './format';
import type { Session } from './queries';

function session(id: string, overrides: Partial<Session> = {}): Session {
  return {
    id,
    label: id,
    cwd: '/repo',
    state: 'idle',
    displayState: 'idle',
    listening: true,
    startedAt: 0,
    lastEventAt: 0,
    runtimeMs: 0,
    client: 'unknown',
    subagents: [],
    tickets: [],
    version: 1,
    ...overrides,
  };
}

const ticket = (key: string) => ({ key, title: key, status: 'backlog' as const });

describe('session format', () => {
  it('reads durations like the legacy list', () => {
    expect(formatDuration(59_000)).toBe('less than 1 min');
    expect(formatDuration(5 * 60_000)).toBe('5 min');
    expect(formatDuration(65 * 60_000)).toBe('1 h 05 min');
    expect(formatDuration(51 * 60 * 60_000)).toBe('2 d 3 h');
  });

  it('says how long ago something happened', () => {
    expect(formatAgo(0, 30_000)).toBe('just now');
    expect(formatAgo(0, 5 * 60_000)).toBe('5 min ago');
    expect(formatAgo(0, 3 * 60 * 60_000)).toBe('3 h ago');
    expect(formatAgo(0, 50 * 60 * 60_000)).toBe('2 d ago');
  });

  it('groups each session once at its lowest ticket, loose ones last', () => {
    const groups = groupByTicket([
      session('loose'),
      session('both', { tickets: [ticket('ARC-0002'), ticket('ARC-0001')] }),
      session('second', { tickets: [ticket('ARC-0002')], displayState: 'running' }),
      session('later', { tickets: [ticket('ARC-0002')], lastEventAt: 5 }),
    ]);
    expect(groups.map((group) => group.ticket?.key ?? null)).toEqual([
      'ARC-0001',
      'ARC-0002',
      null,
    ]);
    expect(groups[1]?.sessions.map((each) => each.id)).toEqual(['second', 'later']);
  });

  it('summarizes the states that need attention and counts runtime to now', () => {
    expect(
      summarize([
        session('a', { displayState: 'running' }),
        session('b', { displayState: 'stale' }),
        session('c', { displayState: 'stale' }),
      ]),
    ).toBe('1 running, 2 stale');
    expect(summarize([session('a')])).toBe('');
    expect(sessionRuntime(session('a', { startedAt: 10 }), 70)).toBe(60);
    expect(sessionRuntime(session('a', { startedAt: 10, endedAt: 30 }), 70)).toBe(20);
  });
});
