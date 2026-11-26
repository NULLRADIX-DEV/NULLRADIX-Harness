import { describe, expect, it } from 'vitest';
import { transcriptLine as line } from '../../test/runFixtures';
import { foldResults } from './transcriptLines';

describe('foldResults', () => {
  it('colors a call by its result and drops the result line', () => {
    const items = foldResults([
      line(1, 'run', 'cargo test', { reference: 'a' }),
      line(2, 'fail', 'exit 1', { reference: 'a' }),
      line(3, 'read', 'src/lib.rs', { reference: 'b' }),
      line(4, 'ok', 'done', { reference: 'b' }),
    ]);
    expect(items.map((item) => [item.line.seq, item.outcome, item.failure])).toEqual([
      [1, 'fail', 'exit 1'],
      [3, 'ok', null],
    ]);
  });

  it('keeps a result without its call and a call without its result', () => {
    const items = foldResults([
      line(1, 'ok', 'lonely', { reference: 'x' }),
      line(2, 'say', 'hello'),
      line(3, 'run', 'pending', { reference: 'y' }),
    ]);
    expect(items.map((item) => [item.line.seq, item.outcome])).toEqual([
      [1, null],
      [2, null],
      [3, null],
    ]);
  });
});
