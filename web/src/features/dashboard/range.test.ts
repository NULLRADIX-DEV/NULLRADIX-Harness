import { describe, expect, it } from 'vitest';
import { filterParams, parseFilter, startOf, statsParams, withEdge } from './range';

const TODAY = new Date(2026, 10, 26, 15, 30);

const BOARD = '0190a0b0-0000-7000-8000-0000000000b1';

function parse(query: string, boardIds?: readonly string[]) {
  return parseFilter(new URLSearchParams(query), TODAY, boardIds);
}

describe('parseFilter', () => {
  it('defaults to all boards, work and 56 days', () => {
    expect(parse('')).toEqual({ boardId: null, kind: 'work', period: '56' });
  });

  it('reads board, kind and a preset period', () => {
    expect(parse(`board=${BOARD}&kind=merge&period=7`)).toEqual({
      boardId: BOARD,
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

describe('parseFilter limits', () => {
  it('keeps only board ids shaped like a UUID', () => {
    expect(parse('board=b1').boardId).toBeNull();
    expect(parse(`board=${BOARD}`).boardId).toBe(BOARD);
  });

  it('drops a board the loaded boards do not hold', () => {
    expect(parse(`board=${BOARD}`, []).boardId).toBeNull();
    expect(parse(`board=${BOARD}`, [BOARD]).boardId).toBe(BOARD);
  });

  it('clamps a custom range to today and to a year', () => {
    expect(parse('period=custom&from=2026-11-01&to=2027-05-01')).toMatchObject({
      from: '2026-11-01',
      to: '2026-11-26',
    });
    expect(parse('period=custom&from=2020-01-01&to=2026-11-10')).toMatchObject({
      from: '2025-11-10',
      to: '2026-11-10',
    });
    expect(parse('period=custom&from=2027-01-01&to=2027-02-01')).toMatchObject({
      from: '2026-11-26',
      to: '2026-11-26',
    });
  });
});

describe('filterParams', () => {
  it('leaves the defaults out and round-trips the rest', () => {
    expect(filterParams(parse('')).toString()).toBe('');
    const filter = parse(
      `board=${BOARD}&kind=research&period=custom&from=2026-11-01&to=2026-11-10`,
    );
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
    });
  });

  it('leaves `to` out for the presets so the daemon uses its own today', () => {
    expect(statsParams(parse(''), TODAY)).not.toHaveProperty('to');
    expect(statsParams(parse('period=config'), TODAY)).not.toHaveProperty('to');
  });

  it('asks since the config change with `config`', () => {
    expect(statsParams(parse('period=config&kind=merge'), TODAY)).toEqual({
      boardId: null,
      kind: 'merge',
      from: 'config',
    });
  });

  it('sends a custom range as it is', () => {
    expect(
      statsParams(parse(`board=${BOARD}&period=custom&from=2026-01-01&to=2026-02-01`), TODAY),
    ).toEqual({ boardId: BOARD, kind: 'work', from: '2026-01-01', to: '2026-02-01' });
  });
});

describe('withEdge', () => {
  const range = { from: '2026-11-01', to: '2026-11-10' };
  const TO = '2026-11-26';

  it('moves one edge', () => {
    expect(withEdge(range, 'from', '2026-11-05', TO)).toEqual({
      from: '2026-11-05',
      to: '2026-11-10',
    });
    expect(withEdge(range, 'to', '2026-11-20', TO)).toEqual({
      from: '2026-11-01',
      to: '2026-11-20',
    });
  });

  it('pulls the other edge along when they would cross', () => {
    expect(withEdge(range, 'from', '2026-11-15', TO)).toEqual({
      from: '2026-11-15',
      to: '2026-11-15',
    });
    expect(withEdge(range, 'to', '2026-10-15', TO)).toEqual({
      from: '2026-10-15',
      to: '2026-10-15',
    });
  });

  it('keeps the range within today and a year', () => {
    expect(withEdge(range, 'to', '2027-01-01', TO)).toEqual({ from: '2026-11-01', to: TO });
    expect(withEdge(range, 'from', '2020-01-01', TO)).toEqual({
      from: '2025-11-10',
      to: '2026-11-10',
    });
  });

  it('ignores a cleared field', () => {
    expect(withEdge(range, 'from', '', TO)).toBe(range);
  });
});
