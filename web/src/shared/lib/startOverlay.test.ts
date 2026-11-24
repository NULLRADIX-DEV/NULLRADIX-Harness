import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { START_OVERLAY_FADE_MS, START_OVERLAY_ID, dismissStartOverlay } from './startOverlay';

describe('dismissStartOverlay', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = `<div id="${START_OVERLAY_ID}"></div><div id="root" inert></div>`;
  });

  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = '';
  });

  it('unblocks the app at once and removes the overlay after the fade', () => {
    dismissStartOverlay();
    expect(document.getElementById('root')).not.toHaveAttribute('inert');
    const overlay = document.getElementById(START_OVERLAY_ID);
    expect(overlay).toHaveClass('is-done');

    vi.advanceTimersByTime(START_OVERLAY_FADE_MS);

    expect(document.getElementById(START_OVERLAY_ID)).toBeNull();
  });

  it('does nothing without an overlay', () => {
    document.getElementById(START_OVERLAY_ID)?.remove();
    expect(() => {
      dismissStartOverlay();
    }).not.toThrow();
    expect(document.getElementById('root')).not.toHaveAttribute('inert');
  });
});
