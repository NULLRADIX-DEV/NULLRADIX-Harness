import { StrictMode } from 'react';
import { QueryClient } from '@tanstack/react-query';
import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useTabKeySync } from '../../app/useTabKeySync';
import { fakeApi, json } from '../../test/fakeApi';
import { api } from './client';
import { TAB_CHANNEL, closeTabChannel } from './tabChannel';
import {
  ASK_WAIT_MS,
  KEY_HEADER,
  adoptTabKey,
  askForTabKey,
  onTabKey,
  readTabKey,
  shareTabKey,
} from './tabKey';

const KEY = 'ab'.repeat(32);
const OTHER_KEY = 'cd'.repeat(32);

// Channels of one name hear each other, never themselves, like the tabs of one origin.
class FakeChannel extends EventTarget {
  static all: FakeChannel[] = [];
  readonly sent: unknown[] = [];

  constructor(readonly name: string) {
    super();
    FakeChannel.all.push(this);
  }

  postMessage(message: unknown) {
    this.sent.push(message);
    for (const other of FakeChannel.all) {
      if (other !== this && other.name === this.name) {
        other.dispatchEvent(new MessageEvent('message', { data: message }));
      }
    }
  }

  close() {
    FakeChannel.all = FakeChannel.all.filter((channel) => channel !== this);
  }
}

function open(hash: string) {
  window.history.replaceState(null, '', `/board?x=1${hash}`);
}

beforeEach(() => {
  closeTabChannel();
  FakeChannel.all = [];
  vi.stubGlobal('BroadcastChannel', FakeChannel);
  window.sessionStorage.clear();
});

afterEach(() => {
  closeTabChannel();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  window.sessionStorage.clear();
  window.history.replaceState(null, '', '/');
});

describe('the key of a tab', () => {
  it('moves from the address into the session storage and leaves the rest of the address', () => {
    open(`#k=${KEY}`);
    expect(adoptTabKey()).toBe(true);
    expect(readTabKey()).toBe(KEY);
    expect(window.location.hash).toBe('');
    expect(window.location.pathname + window.location.search).toBe('/board?x=1');
  });

  it('ignores anything that is not a key', () => {
    for (const hash of ['', '#k=nope', `#k=${KEY}00`, `#other=${KEY}`, `#x#k=${KEY}`]) {
      open(hash);
      expect(adoptTabKey()).toBe(false);
      expect(readTabKey()).toBeNull();
      expect(window.location.hash).toBe(hash === '' ? '' : hash);
    }
  });

  it('is not read from a stored value that is no key', () => {
    window.sessionStorage.setItem('nullradix-tab-key', 'junk');
    expect(readTabKey()).toBeNull();
  });

  it('is sent with every call of the client once a tab has it', async () => {
    const daemon = fakeApi({ 'GET /api/v1/health': () => json(200, {}) });
    await api.GET('/api/v1/health');
    expect(daemon.requests('GET /api/v1/health')[0]?.headers.has(KEY_HEADER)).toBe(false);
    open(`#k=${KEY}`);
    adoptTabKey();
    await api.GET('/api/v1/health');
    await api.POST('/api/v1/auth/logout');
    expect(daemon.requests('GET /api/v1/health')[1]?.headers.get(KEY_HEADER)).toBe(KEY);
    expect(daemon.requests('POST /api/v1/auth/logout')[0]?.headers.get(KEY_HEADER)).toBe(KEY);
  });
});

describe('the key shared between tabs', () => {
  function peer() {
    // the module opens its channel on first use and the other tab follows it
    return new FakeChannel(TAB_CHANNEL);
  }

  it('is announced to the other tabs when it comes from the address', () => {
    const other = peer();
    const heard: unknown[] = [];
    other.addEventListener('message', (event) => heard.push((event as MessageEvent).data));
    open(`#k=${KEY}`);
    adoptTabKey();
    expect(heard).toEqual([{ type: 'key', key: KEY }]);
  });

  it('is taken over when another tab announces a different one, and the tab is told', () => {
    const stop = shareTabKey();
    const other = peer();
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const told: string[] = [];
    const off = onTabKey((key) => told.push(key));
    other.postMessage({ type: 'key', key: OTHER_KEY });
    expect(readTabKey()).toBe(OTHER_KEY);
    expect(told).toEqual([OTHER_KEY]);
    // the same key again changes nothing and tells nobody
    other.postMessage({ type: 'key', key: OTHER_KEY });
    expect(told).toEqual([OTHER_KEY]);
    off();
    stop();
  });

  it('does not take a message that carries no key', () => {
    const stop = shareTabKey();
    const other = peer();
    for (const data of [
      { type: 'key', key: 'nope' },
      { type: 'key' },
      { type: 'key', key: 7 },
      'key',
    ]) {
      other.postMessage(data);
    }
    expect(readTabKey()).toBeNull();
    stop();
  });

  it('answers a tab that asks, but only with a key it has', () => {
    const stop = shareTabKey();
    const other = peer();
    const heard: unknown[] = [];
    other.addEventListener('message', (event) => heard.push((event as MessageEvent).data));
    other.postMessage({ type: 'ask' });
    expect(heard).toEqual([]);
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    other.postMessage({ type: 'ask' });
    expect(heard).toEqual([{ type: 'key', key: KEY }]);
    stop();
    other.postMessage({ type: 'ask' });
    expect(heard).toHaveLength(1);
  });

  it('is asked for by a tab without one, which keeps what the other tab answers', async () => {
    const stop = shareTabKey();
    const other = peer();
    other.addEventListener('message', (event) => {
      if ((event as MessageEvent<{ type: string }>).data.type === 'ask') {
        other.postMessage({ type: 'key', key: KEY });
      }
    });
    const found = await askForTabKey();
    expect(found).toBe(true);
    expect(readTabKey()).toBe(KEY);
    stop();
  });

  it('gives up after a short wait when no tab answers', async () => {
    vi.useFakeTimers();
    const other = peer();
    const asked: unknown[] = [];
    other.addEventListener('message', (event) => asked.push((event as MessageEvent).data));
    const waiting = askForTabKey();
    expect(asked).toEqual([{ type: 'ask' }]);
    await vi.advanceTimersByTimeAsync(ASK_WAIT_MS);
    expect(await waiting).toBe(false);
    expect(readTabKey()).toBeNull();
    // a late answer is not kept by the finished ask
    other.postMessage({ type: 'key', key: KEY });
    expect(readTabKey()).toBeNull();
  });

  it('does not count the answer that is the key it already has', async () => {
    vi.useFakeTimers();
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const other = peer();
    other.addEventListener('message', (event) => {
      if ((event as MessageEvent<{ type: string }>).data.type === 'ask') {
        other.postMessage({ type: 'key', key: KEY });
      }
    });
    const waiting = askForTabKey();
    await vi.advanceTimersByTimeAsync(ASK_WAIT_MS);
    expect(await waiting).toBe(false);
  });

  it('does not ask when the browser has no channel', async () => {
    vi.stubGlobal('BroadcastChannel', undefined);
    closeTabChannel();
    expect(await askForTabKey()).toBe(false);
  });
});

describe('the key under React StrictMode', () => {
  it('stays when the address is read twice, and is announced once', () => {
    const other = new FakeChannel(TAB_CHANNEL);
    const heard: unknown[] = [];
    other.addEventListener('message', (event) => heard.push((event as MessageEvent).data));
    open(`#k=${KEY}`);
    expect(adoptTabKey()).toBe(true);
    // the second read finds no fragment and must keep what the first one stored
    expect(adoptTabKey()).toBe(false);
    expect(readTabKey()).toBe(KEY);
    expect(heard).toEqual([{ type: 'key', key: KEY }]);
  });

  it('is announced to the page once when the sync hook runs its effect twice', () => {
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    const queryClient = new QueryClient();
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');
    renderHook(
      () => {
        useTabKeySync(queryClient);
      },
      { wrapper: StrictMode },
    );
    act(() => {
      other.postMessage({ type: 'key', key: KEY });
    });
    expect(invalidate).toHaveBeenCalledTimes(1);
    expect(readTabKey()).toBe(KEY);
    stop();
  });

  it('stops listening when the sync hook goes away', () => {
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    const queryClient = new QueryClient();
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');
    const view = renderHook(
      () => {
        useTabKeySync(queryClient);
      },
      { wrapper: StrictMode },
    );
    view.unmount();
    other.postMessage({ type: 'key', key: KEY });
    expect(invalidate).not.toHaveBeenCalled();
    stop();
  });
});
