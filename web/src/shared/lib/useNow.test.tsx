import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TICK_MS, useNow } from './useNow';

function Clock({ ticking }: { ticking: boolean }) {
  return <p>{useNow(ticking)}</p>;
}

describe('useNow', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('renews the time only while ticking', () => {
    const { rerender } = render(<Clock ticking />);
    expect(screen.getByText('1000000')).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(TICK_MS);
    });
    expect(screen.getByText(String(1_000_000 + TICK_MS))).toBeInTheDocument();

    rerender(<Clock ticking={false} />);
    act(() => {
      vi.advanceTimersByTime(TICK_MS * 3);
    });
    expect(screen.getByText(String(1_000_000 + TICK_MS))).toBeInTheDocument();
  });
});
