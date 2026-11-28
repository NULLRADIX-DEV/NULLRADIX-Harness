import { StrictMode } from 'react';
import { QueryClient } from '@tanstack/react-query';
import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useTabKeySync } from '../../app/useTabKeySync';
import { fakeApi, json, problem } from '../../test/fakeApi';
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
const STALE_KEY = 'ef'.repeat(32);

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

// A daemon whose session accepts only these keys together with the cookie of the browser.
function daemonKnowing(...keys: string[]) {
  return fakeApi({
    'GET /api/v1/auth/me': (request) =>
      keys.includes(request.headers.get(KEY_HEADER) ?? '')
        ? json(200, { kind: 'session', elevatedUntil: null })
        : problem(401, 'session-key-invalid', 'the key is wrong'),
  });
}

// Lets the checks of announced keys finish.
async function settle() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

function listen(channel: FakeChannel): unknown[] {
  const heard: unknown[] = [];
  channel.addEventListener('message', (event) => heard.push((event as MessageEvent).data));
  return heard;
}

const stored = () => window.sessionStorage.getItem('nullradix-tab-key');

describe('the key of a tab', () => {
  it('moves from the address into the session storage once the daemon vouches for it', async () => {
    const daemon = daemonKnowing(KEY);
    open(`#k=${KEY}`);
    const adopted = adoptTabKey();
    // the address is clear before the check ends and the router never sees the key
    expect(window.location.hash).toBe('');
    expect(window.location.pathname + window.location.search).toBe('/board?x=1');
    expect(await adopted).toBe(true);
    expect(readTabKey()).toBe(KEY);
    expect(daemon.requests('GET /api/v1/auth/me')[0]?.headers.get(KEY_HEADER)).toBe(KEY);
  });

  it('is neither kept nor announced when the daemon refuses it', async () => {
    daemonKnowing(KEY);
    const other = new FakeChannel(TAB_CHANNEL);
    const heard = listen(other);
    open(`#k=${OTHER_KEY}`);
    expect(await adoptTabKey()).toBe(false);
    expect(readTabKey()).toBeNull();
    expect(heard).toEqual([]);
    expect(window.location.hash).toBe('');
  });

  it('is not kept when the daemon cannot be asked', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('offline')));
    open(`#k=${KEY}`);
    expect(await adoptTabKey()).toBe(false);
    expect(readTabKey()).toBeNull();
  });

  it('ignores anything that is not a key', async () => {
    const daemon = daemonKnowing(KEY);
    for (const hash of ['', '#k=nope', `#k=${KEY}00`, `#other=${KEY}`, `#x#k=${KEY}`]) {
      open(hash);
      expect(await adoptTabKey()).toBe(false);
      expect(readTabKey()).toBeNull();
      expect(window.location.hash).toBe(hash === '' ? '' : hash);
    }
    expect(daemon.requests('GET /api/v1/auth/me')).toHaveLength(0);
  });

  it('is not read from a stored value that is no key', () => {
    window.sessionStorage.setItem('nullradix-tab-key', 'junk');
    expect(readTabKey()).toBeNull();
  });

  it('is sent with every call of the client once a tab has it', async () => {
    const daemon = daemonKnowing(KEY);
    daemon.set('GET /api/v1/health', () => json(200, {}));
    await api.GET('/api/v1/health');
    expect(daemon.requests('GET /api/v1/health')[0]?.headers.has(KEY_HEADER)).toBe(false);
    open(`#k=${KEY}`);
    await adoptTabKey();
    await api.GET('/api/v1/health');
    await api.POST('/api/v1/auth/logout');
    expect(daemon.requests('GET /api/v1/health')[1]?.headers.get(KEY_HEADER)).toBe(KEY);
    expect(daemon.requests('POST /api/v1/auth/logout')[0]?.headers.get(KEY_HEADER)).toBe(KEY);
  });
});

describe('the key shared between tabs', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  it('is announced to the other tabs when it comes from the address', async () => {
    daemonKnowing(KEY);
    const other = new FakeChannel(TAB_CHANNEL);
    const heard = listen(other);
    open(`#k=${KEY}`);
    expect(await adoptTabKey()).toBe(true);
    expect(heard).toEqual([{ type: 'key', key: KEY }]);
  });

  it('is taken over when another tab announces a different key the daemon vouches for', async () => {
    const daemon = daemonKnowing(OTHER_KEY);
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const told: string[] = [];
    const off = onTabKey((key) => told.push(key));
    other.postMessage({ type: 'key', key: OTHER_KEY });
    await settle();
    expect(readTabKey()).toBe(OTHER_KEY);
    expect(told).toEqual([OTHER_KEY]);
    // the same key again changes nothing, asks nothing and tells nobody
    other.postMessage({ type: 'key', key: OTHER_KEY });
    await settle();
    expect(told).toEqual([OTHER_KEY]);
    expect(daemon.requests('GET /api/v1/auth/me')).toHaveLength(1);
    off();
    stop();
  });

  it('keeps its own key when a bogus one is announced, as a made-up address would', async () => {
    const daemon = daemonKnowing(KEY);
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const told: string[] = [];
    const off = onTabKey((key) => told.push(key));
    other.postMessage({ type: 'key', key: OTHER_KEY });
    await settle();
    expect(readTabKey()).toBe(KEY);
    expect(told).toEqual([]);
    expect(daemon.requests('GET /api/v1/auth/me')).toHaveLength(1);
    off();
    stop();
  });

  it('does not take a bogus key either when it has none', async () => {
    daemonKnowing(KEY);
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    other.postMessage({ type: 'key', key: OTHER_KEY });
    await settle();
    expect(readTabKey()).toBeNull();
    stop();
  });

  it('keeps the fresh key when a stale one is announced after it', async () => {
    // a second nullradix open ended the session of the old key
    daemonKnowing(OTHER_KEY);
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    window.sessionStorage.setItem('nullradix-tab-key', STALE_KEY);
    other.postMessage({ type: 'key', key: OTHER_KEY });
    await settle();
    other.postMessage({ type: 'key', key: STALE_KEY });
    await settle();
    expect(readTabKey()).toBe(OTHER_KEY);
    stop();
  });

  it('does not take a message that carries no key', async () => {
    const daemon = daemonKnowing(KEY);
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    for (const data of [
      { type: 'key', key: 'nope' },
      { type: 'key' },
      { type: 'key', key: 7 },
      { type: 'answer', to: 'x', key: 'nope' },
      'key',
    ]) {
      other.postMessage(data);
    }
    await settle();
    expect(readTabKey()).toBeNull();
    expect(daemon.requests('GET /api/v1/auth/me')).toHaveLength(0);
    stop();
  });

  it('answers a tab that asks to that tab alone, and only with a key it has', () => {
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    const heard = listen(other);
    other.postMessage({ type: 'ask', nonce: 'n1' });
    expect(heard).toEqual([]);
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    other.postMessage({ type: 'ask', nonce: 'n2' });
    expect(heard).toEqual([{ type: 'answer', to: 'n2', key: KEY }]);
    stop();
    other.postMessage({ type: 'ask', nonce: 'n3' });
    expect(heard).toHaveLength(1);
  });

  it('keeps what a tab without a key is answered, once the daemon vouches for it', async () => {
    daemonKnowing(KEY);
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    other.addEventListener('message', (event) => {
      const data = (event as MessageEvent<{ type: string; nonce?: string }>).data;
      if (data.type === 'ask') other.postMessage({ type: 'answer', to: data.nonce, key: KEY });
    });
    const asking = askForTabKey();
    await settle();
    expect(await asking).toBe(true);
    expect(readTabKey()).toBe(KEY);
    stop();
  });

  it('takes no answer meant for another asking tab, nor one nobody asked for', async () => {
    const daemon = daemonKnowing(KEY);
    const stop = shareTabKey();
    const other = new FakeChannel(TAB_CHANNEL);
    // nobody asks: an unsolicited answer is ignored
    other.postMessage({ type: 'answer', to: 'nobody', key: KEY });
    await settle();
    expect(readTabKey()).toBeNull();
    const asked = listen(other);
    const asking = askForTabKey();
    const { nonce } = asked[0] as { nonce: string };
    other.postMessage({ type: 'answer', to: `${nonce}-of-another-tab`, key: KEY });
    await settle();
    expect(readTabKey()).toBeNull();
    await vi.advanceTimersByTimeAsync(ASK_WAIT_MS);
    expect(await asking).toBe(false);
    expect(daemon.requests('GET /api/v1/auth/me')).toHaveLength(0);
    stop();
  });

  it('keeps the fresh answer and drops a stale one that comes after it', async () => {
    daemonKnowing(OTHER_KEY);
    const other = new FakeChannel(TAB_CHANNEL);
    const asked = listen(other);
    const asking = askForTabKey();
    const { nonce } = asked[0] as { nonce: string };
    other.postMessage({ type: 'answer', to: nonce, key: OTHER_KEY });
    await settle();
    other.postMessage({ type: 'answer', to: nonce, key: STALE_KEY });
    await settle();
    expect(await asking).toBe(true);
    expect(readTabKey()).toBe(OTHER_KEY);
  });

  it('goes on waiting after a stale answer and takes the fresh one that follows', async () => {
    daemonKnowing(OTHER_KEY);
    const other = new FakeChannel(TAB_CHANNEL);
    const asked = listen(other);
    const asking = askForTabKey();
    const { nonce } = asked[0] as { nonce: string };
    other.postMessage({ type: 'answer', to: nonce, key: STALE_KEY });
    await settle();
    other.postMessage({ type: 'answer', to: nonce, key: OTHER_KEY });
    await settle();
    expect(await asking).toBe(true);
    expect(readTabKey()).toBe(OTHER_KEY);
  });

  it('gives up after a short wait when no tab answers', async () => {
    const other = new FakeChannel(TAB_CHANNEL);
    const asked = listen(other);
    const asking = askForTabKey();
    expect(asked).toEqual([{ type: 'ask', nonce: expect.any(String) as string }]);
    await vi.advanceTimersByTimeAsync(ASK_WAIT_MS);
    expect(await asking).toBe(false);
    expect(readTabKey()).toBeNull();
  });

  it('does not count the answer that is the key it already has', async () => {
    daemonKnowing(KEY);
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const other = new FakeChannel(TAB_CHANNEL);
    const asked = listen(other);
    const asking = askForTabKey();
    const { nonce } = asked[0] as { nonce: string };
    other.postMessage({ type: 'answer', to: nonce, key: KEY });
    await vi.advanceTimersByTimeAsync(ASK_WAIT_MS);
    expect(await asking).toBe(false);
  });

  it('does not ask when the browser has no channel', async () => {
    vi.stubGlobal('BroadcastChannel', undefined);
    closeTabChannel();
    expect(await askForTabKey()).toBe(false);
  });
});

describe('the key under React StrictMode', () => {
  it('stays when the address is read twice, and is announced once', async () => {
    daemonKnowing(KEY);
    const other = new FakeChannel(TAB_CHANNEL);
    const heard = listen(other);
    open(`#k=${KEY}`);
    const first = adoptTabKey();
    // the second read finds no fragment and must keep what the first one stored
    expect(await adoptTabKey()).toBe(false);
    expect(await first).toBe(true);
    expect(readTabKey()).toBe(KEY);
    expect(heard).toEqual([{ type: 'key', key: KEY }]);
  });

  it('is announced to the page once when the sync hook runs its effect twice', async () => {
    vi.useFakeTimers();
    daemonKnowing(KEY);
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
    other.postMessage({ type: 'key', key: KEY });
    await settle();
    expect(invalidate).toHaveBeenCalledTimes(1);
    expect(readTabKey()).toBe(KEY);
    expect(stored()).toBe(KEY);
    stop();
  });

  it('stops listening when the sync hook goes away', async () => {
    vi.useFakeTimers();
    daemonKnowing(KEY);
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
    await settle();
    expect(invalidate).not.toHaveBeenCalled();
    stop();
  });
});
