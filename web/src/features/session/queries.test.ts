import { QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { KEY_HEADER, readTabKey } from '../../shared/api/tabKey';
import { TAB_CHANNEL, closeTabChannel } from '../../shared/api/tabChannel';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { sessionQuery } from './queries';

const OLD_KEY = 'ab'.repeat(32);
const NEW_KEY = 'cd'.repeat(32);

// Channels of one name hear each other, never themselves, like the tabs of one origin.
class FakeChannel extends EventTarget {
  static all: FakeChannel[] = [];

  constructor(readonly name: string) {
    super();
    FakeChannel.all.push(this);
  }

  postMessage(message: unknown) {
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

// A daemon knowing only NEW_KEY.
function daemon(code: string) {
  return fakeApi({
    'GET /api/v1/auth/me': (request) =>
      request.headers.get(KEY_HEADER) === NEW_KEY
        ? json(200, { kind: 'session', elevatedUntil: null })
        : problem(401, code, 'the key is wrong'),
  });
}

function otherTabAnswers() {
  const other = new FakeChannel(TAB_CHANNEL);
  other.addEventListener('message', (event) => {
    const data = (event as MessageEvent<{ type: string; nonce?: string }>).data;
    if (data.type === 'ask') other.postMessage({ type: 'answer', to: data.nonce, key: NEW_KEY });
  });
}

beforeEach(() => {
  closeTabChannel();
  FakeChannel.all = [];
  vi.stubGlobal('BroadcastChannel', FakeChannel);
  window.sessionStorage.setItem('nullradix-tab-key', OLD_KEY);
});

afterEach(() => {
  closeTabChannel();
  vi.unstubAllGlobals();
  window.sessionStorage.clear();
});

describe('sessionQuery', () => {
  it.each(['session-key-invalid', 'session-key-missing'])(
    'asks the other tabs for the current key after %s and checks again',
    async (code) => {
      const api = daemon(code);
      otherTabAnswers();
      const session = await new QueryClient().query(sessionQuery());
      expect(session).toEqual({ kind: 'session', elevatedUntil: null });
      expect(readTabKey()).toBe(NEW_KEY);
      // the refused check, the vouching check of the new key and the retry
      expect(api.requests('GET /api/v1/auth/me')).toHaveLength(3);
    },
  );

  it('is signed out when no other tab has a newer key', async () => {
    const api = daemon('session-key-invalid');
    otherTabAnswers();
    // the answer repeats the key this tab has and is no news
    window.sessionStorage.setItem('nullradix-tab-key', NEW_KEY);
    api.set('GET /api/v1/auth/me', () => problem(401, 'session-key-invalid', 'the key is wrong'));
    const session = await new QueryClient().query(sessionQuery());
    expect(session).toBeNull();
    expect(api.requests('GET /api/v1/auth/me')).toHaveLength(1);
  });

  it('does not ask the other tabs for any other refusal', async () => {
    const api = fakeApi({
      'GET /api/v1/auth/me': () => problem(401, 'credentials-missing', 'sign in first'),
    });
    otherTabAnswers();
    expect(await new QueryClient().query(sessionQuery())).toBeNull();
    expect(api.requests('GET /api/v1/auth/me')).toHaveLength(1);
    expect(readTabKey()).toBe(OLD_KEY);
  });
});
