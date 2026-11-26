import { describe, expect, it } from 'vitest';
import { filterParams, parseFilter, startOf, statsParams, withEdge } from './range';

const TODAY = new Date(2026, 10, 26, 15, 30);

function parse(query: string) {
  return parseFilter(new URLSearchParams(query));
}

describe('parseFilter', () => {
  it('defaults to all boards, work and 56 days', () => {
    expect(parse('')).toEqual({ boardId: null, kind: 'work', period: '56' });
  });

  it('reads board, kind and a preset period', () => {
    expect(parse('board=b1&kind=merge&period=7')).toEqual({
      boardId: 'b1',
      kind: 'merge',
      period: '7',
    });
    expect(parse('period=config').period).toBe('config');
  });

  it('drops unknown values', () => {
    expect(parse('kind=plan&period=90')).toEqual({ boardId: null, kind: 'work', period: '56' });
  });

  it('keeps a custom range in order', () => {
    expect(parse('period=custom&from=2026-11-01&to=2026-11-10')).toMatchObject({
      period: 'custom',
      from: '2026-11-01',
      to: '2026-11-10',
    });
    expect(parse('period=custom&from=2026-11-10&to=2026-11-01')).toMatchObject({
      from: '2026-11-01',
      to: '2026-11-10',
    });
  });

  it('falls back to the default when a custom range lacks valid days', () => {
    expect(parse('period=custom')).toEqual({ boardId: null, kind: 'work', period: '56' });
    expect(parse('period=custom&from=x&to=2026-11-01').period).toBe('56');
  });
});

describe('filterParams', () => {
  it('leaves the defaults out and round-trips the rest', () => {
    expect(filterParams(parse('')).toString()).toBe('');
    const filter = parse('board=b1&kind=research&period=custom&from=2026-11-01&to=2026-11-10');
    expect(parse(filterParams(filter).toString())).toEqual(filter);
  });
});

describe('statsParams', () => {
  it('counts the last n days including today', () => {
    expect(startOf(TODAY, 1)).toBe('2026-11-26');
    expect(startOf(TODAY, 7)).toBe('2026-11-20');
    expect(statsParams(parse(''), TODAY)).toEqual({
      boardId: null,
      kind: 'work',
      from: '2026-10-02',
      to: '2026-11-26',
    });
  });

  it('asks since the config change with `config`', () => {
    expect(statsParams(parse('period=config&kind=merge'), TODAY)).toEqual({
      boardId: null,
      kind: 'merge',
      from: 'config',
      to: '2026-11-26',
    });
  });

  it('sends a custom range as it is', () => {
    expect(
      statsParams(parse('board=b1&period=custom&from=2026-01-01&to=2026-02-01'), TODAY),
    ).toEqual({ boardId: 'b1', kind: 'work', from: '2026-01-01', to: '2026-02-01' });
  });
});

describe('withEdge', () => {
  const range = { from: '2026-11-01', to: '2026-11-10' };

  it('moves one edge', () => {
    expect(withEdge(range, 'from', '2026-11-05')).toEqual({ from: '2026-11-05', to: '2026-11-10' });
    expect(withEdge(range, 'to', '2026-11-20')).toEqual({ from: '2026-11-01', to: '2026-11-20' });
  });

  it('pulls the other edge along when they would cross', () => {
    expect(withEdge(range, 'from', '2026-11-15')).toEqual({ from: '2026-11-15', to: '2026-11-15' });
    expect(withEdge(range, 'to', '2026-10-15')).toEqual({ from: '2026-10-15', to: '2026-10-15' });
  });

  it('ignores a cleared field', () => {
    expect(withEdge(range, 'from', '')).toBe(range);
  });
});
