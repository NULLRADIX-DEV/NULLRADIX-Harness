import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CHANGES_REFRESH_MS, useRefreshChangesOnGrowth } from './liveChanges';
import { sessionKeys, sessionsLive } from './queries';

function setup(size: number | null | undefined, sessionId = 's-1') {
  const queryClient = new QueryClient();
  const invalidate = vi.spyOn(queryClient, 'invalidateQueries');
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );
  const view = renderHook(
    (props) => {
      useRefreshChangesOnGrowth(props.id, props.size);
    },
    {
      wrapper,
      initialProps: { id: sessionId, size },
    },
  );
  const keys = () => invalidate.mock.calls.map(([filters]) => filters?.queryKey);
  return { view, keys };
}

describe('useRefreshChangesOnGrowth', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('leaves changes alone while the transcript keeps its size', () => {
    const { view, keys } = setup(100);
    view.rerender({ id: 's-1', size: 100 });
    vi.advanceTimersByTime(CHANGES_REFRESH_MS * 3);
    expect(keys()).toEqual([]);
  });

  it('reads changes and diffs again when it grew', () => {
    const { view, keys } = setup(100);
    view.rerender({ id: 's-1', size: 150 });
    vi.advanceTimersByTime(0);
    expect(keys()).toEqual([sessionKeys.changes('s-1'), sessionKeys.diffs('s-1')]);
  });

  it('reads at most once in the interval and then with the latest growth', () => {
    const { view, keys } = setup(100);
    view.rerender({ id: 's-1', size: 150 });
    vi.advanceTimersByTime(0);
    expect(keys()).toHaveLength(2);
    view.rerender({ id: 's-1', size: 200 });
    vi.advanceTimersByTime(CHANGES_REFRESH_MS - 1);
    expect(keys()).toHaveLength(2);
    view.rerender({ id: 's-1', size: 250 });
    vi.advanceTimersByTime(1);
    expect(keys()).toHaveLength(4);
    vi.advanceTimersByTime(CHANGES_REFRESH_MS * 3);
    expect(keys()).toHaveLength(4);
  });

  it('starts over for another session and waits for a known size', () => {
    const { view, keys } = setup(undefined);
    view.rerender({ id: 's-1', size: 100 });
    view.rerender({ id: 's-2', size: 900 });
    vi.advanceTimersByTime(CHANGES_REFRESH_MS);
    expect(keys()).toEqual([]);
    view.rerender({ id: 's-2', size: 901 });
    vi.advanceTimersByTime(0);
    expect(keys()[0]).toEqual(sessionKeys.changes('s-2'));
  });
});

describe('sessionsLive', () => {
  it('reloads the lists on a session change and never the changes of an open detail', () => {
    const keys = sessionsLive['session'] ?? [];
    expect(keys).toEqual([sessionKeys.list(), sessionKeys.tickets()]);
    for (const key of [sessionKeys.changes('s-1'), sessionKeys.diff('s-1', 'a.rs')]) {
      expect(
        [...keys, ...(sessionsLive['ticket'] ?? [])].some((prefix) =>
          prefix.every((part, index) => key[index] === part),
        ),
      ).toBe(false);
    }
  });
});
