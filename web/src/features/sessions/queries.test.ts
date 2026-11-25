import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fakeApi, json } from '../../test/fakeApi';
import { ENDED_REFRESH_MS, LIVE_REFRESH_MS, sessionDetailQuery } from './queries';

const ID = 's-1';

function detail(displayState: 'ended' | 'idle') {
  return { session: { id: ID, displayState } };
}

describe('sessionDetailQuery', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('keeps a slow look at an ended session and notices its resume', async () => {
    let state: 'ended' | 'idle' = 'ended';
    const api = fakeApi({
      [`GET /api/v1/sessions/${ID}/detail`]: () => json(200, detail(state)),
    });
    const count = () => api.requests(`GET /api/v1/sessions/${ID}/detail`).length;
    const client = new QueryClient();
    const observer = new QueryObserver(client, sessionDetailQuery(ID));
    const stop = observer.subscribe(() => undefined);
    await vi.advanceTimersByTimeAsync(0);
    expect(count()).toBe(1);
    expect(observer.getCurrentResult().data?.session.displayState).toBe('ended');

    await vi.advanceTimersByTimeAsync(LIVE_REFRESH_MS * 2);
    expect(count()).toBe(1);

    state = 'idle';
    await vi.advanceTimersByTimeAsync(ENDED_REFRESH_MS - LIVE_REFRESH_MS * 2);
    expect(count()).toBe(2);
    expect(observer.getCurrentResult().data?.session.displayState).toBe('idle');

    await vi.advanceTimersByTimeAsync(LIVE_REFRESH_MS);
    expect(count()).toBe(3);
    stop();
  });
});
