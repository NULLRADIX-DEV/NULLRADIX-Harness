import { describe, expect, it } from 'vitest';
import { board } from '../../test/boardServer';
import { groupByProject, pickBoard, readBoardChoice, saveBoardChoice } from './boardChoice';

const blocked: Pick<Storage, 'getItem' | 'setItem'> = {
  getItem: () => {
    throw new Error('blocked');
  },
  setItem: () => {
    throw new Error('blocked');
  },
};

describe('boardChoice', () => {
  it('picks the chosen board while it exists, else the first', () => {
    const first = board({ id: 'a' });
    const second = board({ id: 'b' });
    expect(pickBoard([first, second], 'b')).toBe(second);
    expect(pickBoard([first, second], 'gone')).toBe(first);
    expect(pickBoard([], 'b')).toBeUndefined();
  });

  it('groups boards by project in the given order', () => {
    const boards = [
      board({ id: 'a', projectKey: 'ARC' }),
      board({ id: 'b', projectKey: 'NX' }),
      board({ id: 'c', projectKey: 'ARC' }),
    ];
    expect(
      groupByProject(boards).map((group) => [group.projectKey, group.boards.map((b) => b.id)]),
    ).toEqual([
      ['ARC', ['a', 'c']],
      ['NX', ['b']],
    ]);
  });

  it('survives blocked storage', () => {
    expect(readBoardChoice(blocked)).toBeNull();
    expect(() => {
      saveBoardChoice(blocked, 'a');
    }).not.toThrow();
  });
});
