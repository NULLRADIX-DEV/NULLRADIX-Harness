import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { fakeApi, json } from '../../test/fakeApi';
import { api } from './client';
import { KEY_HEADER, adoptTabKey, readTabKey } from './tabKey';

const KEY = 'ab'.repeat(32);

function open(hash: string) {
  window.history.replaceState(null, '', `/board?x=1${hash}`);
}

beforeEach(() => {
  window.sessionStorage.clear();
});

afterEach(() => {
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
