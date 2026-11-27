import { act, renderHook } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { BOARD_STORAGE_KEY } from './boardChoice';
import { chooseBoardId, useBoardChoice } from './useBoardChoice';

describe('useBoardChoice', () => {
  it('follows a choice made anywhere and one made in another tab', () => {
    window.localStorage.setItem(BOARD_STORAGE_KEY, 'one');
    const { result } = renderHook(() => useBoardChoice());
    expect(result.current).toBe('one');
    act(() => {
      chooseBoardId('two');
    });
    expect(result.current).toBe('two');
    expect(window.localStorage.getItem(BOARD_STORAGE_KEY)).toBe('two');
    act(() => {
      window.localStorage.setItem(BOARD_STORAGE_KEY, 'three');
      window.dispatchEvent(new StorageEvent('storage', { key: BOARD_STORAGE_KEY }));
    });
    expect(result.current).toBe('three');
  });

  it('keeps the choice for this visit while the storage is blocked', () => {
    const { result } = renderHook(() => useBoardChoice());
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    act(() => {
      chooseBoardId('four');
    });
    expect(result.current).toBe('four');
  });
});
