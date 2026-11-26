import { describe, expect, it } from 'vitest';
import { ticketsLive } from '../tickets/queries';
import { reviewKeys } from './queries';

function covers(prefix: readonly unknown[], key: readonly unknown[]): boolean {
  return prefix.every((part, index) => key[index] === part);
}

describe('review keys', () => {
  it('are not reloaded by changes of tickets or comments', () => {
    const keys = [
      reviewKeys.of('ARC-1'),
      reviewKeys.patch('ARC-1', 'src/a.ts'),
      reviewKeys.file('ARC-1', 'src/a.ts'),
    ];
    for (const prefix of Object.values(ticketsLive).flat()) {
      for (const key of keys) expect(covers(prefix, key), JSON.stringify(prefix)).toBe(false);
    }
  });
});
