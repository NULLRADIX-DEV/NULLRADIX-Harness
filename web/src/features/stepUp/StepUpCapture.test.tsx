import { QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createQueryClient } from '../../app/queryClient';
import { KEY_HEADER } from '../../shared/api/tabKey';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { CONFIRM_CHANNEL, parseConfirmCode } from './confirmLink';
import { HAND_ON_WAIT_MS, StepUpCapture } from './StepUpCapture';

const KEY = 'ab'.repeat(32);
const CODE = 'cd'.repeat(32);

class FakeChannel extends EventTarget {
  static all: FakeChannel[] = [];
  readonly sent: unknown[] = [];

  constructor(readonly name: string) {
    super();
    FakeChannel.all.push(this);
  }

  postMessage(message: unknown) {
    this.sent.push(message);
  }

  hear(data: unknown) {
    this.dispatchEvent(new MessageEvent('message', { data }));
  }

  close() {
    FakeChannel.all = FakeChannel.all.filter((channel) => channel !== this);
  }
}

function show(hash: string) {
  window.history.replaceState(null, '', `/settings${hash}`);
}

function mount(first: string | null = null) {
  return render(
    <QueryClientProvider client={createQueryClient({ retry: false })}>
      <StepUpCapture first={first} />
    </QueryClientProvider>,
  );
}

const ELEVATED = { elevatedUntil: Date.now() + 600_000 };

beforeEach(() => {
  FakeChannel.all = [];
  vi.stubGlobal('BroadcastChannel', FakeChannel);
  window.sessionStorage.clear();
});

afterEach(() => {
  window.sessionStorage.clear();
  window.history.replaceState(null, '', '/');
  vi.useRealTimers();
});

describe('parseConfirmCode', () => {
  it('reads only a code in the fragment', () => {
    expect(parseConfirmCode(`#confirm=${CODE}`)).toBe(CODE);
    for (const hash of ['', '#confirm=', '#confirm=nope', `#k=${CODE}`, `#confirm=${CODE}x`]) {
      expect(parseConfirmCode(hash)).toBeNull();
    }
  });
});

describe('StepUpCapture', () => {
  it('redeems the code of the address with the key of the tab and clears the address', async () => {
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const daemon = fakeApi({ 'POST /api/v1/auth/step-up': () => json(200, ELEVATED) });
    show(`#confirm=${CODE}`);
    mount();
    expect(await screen.findByRole('status')).toHaveTextContent('Confirmed for 10 minutes');
    const [request] = daemon.requests('POST /api/v1/auth/step-up');
    expect(request?.headers.get(KEY_HEADER)).toBe(KEY);
    expect(await request?.json()).toEqual({ code: CODE });
    expect(window.location.hash).toBe('');
    expect(window.location.pathname).toBe('/settings');
  });

  it('uses the code taken from the address before the router moved on', async () => {
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const daemon = fakeApi({ 'POST /api/v1/auth/step-up': () => json(200, ELEVATED) });
    show('');
    mount(CODE);
    await waitFor(() => {
      expect(daemon.requests('POST /api/v1/auth/step-up')).toHaveLength(1);
    });
  });

  it('takes a code that arrives later, as the tray menu sets it', async () => {
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const daemon = fakeApi({ 'POST /api/v1/auth/step-up': () => json(200, ELEVATED) });
    show('');
    mount();
    expect(daemon.requests('POST /api/v1/auth/step-up')).toHaveLength(0);
    act(() => {
      window.location.hash = `confirm=${CODE}`;
    });
    await waitFor(() => {
      expect(daemon.requests('POST /api/v1/auth/step-up')).toHaveLength(1);
    });
  });

  it('says so when the code is not accepted', async () => {
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    fakeApi({
      'POST /api/v1/auth/step-up': () => problem(401, 'step-up-code-invalid', 'expired'),
    });
    show(`#confirm=${CODE}`);
    mount();
    expect(await screen.findByRole('status')).toHaveTextContent('unknown or expired');
  });

  it('hands the code on when the tab has no key, and is told when another tab took it', () => {
    const daemon = fakeApi();
    show(`#confirm=${CODE}`);
    mount();
    const [channel] = FakeChannel.all;
    expect(channel?.name).toBe(CONFIRM_CHANNEL);
    expect(channel?.sent).toEqual([{ type: 'code', code: CODE }]);
    expect(daemon.requests('POST /api/v1/auth/step-up')).toHaveLength(0);
    // a refusal of one tab does not end the wait
    act(() => {
      channel?.hear({ type: 'done', code: CODE, ok: false });
    });
    expect(screen.getByRole('status')).toHaveTextContent('Looking for');
    act(() => {
      channel?.hear({ type: 'done', code: CODE, ok: true });
    });
    expect(screen.getByRole('status')).toHaveTextContent('Confirmed in your open NULLRADIX tab');
  });

  it('gives up on a hand-on that nobody takes', () => {
    vi.useFakeTimers();
    fakeApi();
    show(`#confirm=${CODE}`);
    mount();
    act(() => {
      vi.advanceTimersByTime(HAND_ON_WAIT_MS);
    });
    expect(screen.getByRole('status')).toHaveTextContent('No open NULLRADIX tab took the code');
  });

  it('redeems a code another tab hands over, and reports it', async () => {
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const daemon = fakeApi({ 'POST /api/v1/auth/step-up': () => json(200, ELEVATED) });
    show('');
    mount();
    const [channel] = FakeChannel.all;
    act(() => {
      channel?.hear({ type: 'code', code: CODE });
    });
    await waitFor(() => {
      expect(channel?.sent).toEqual([{ type: 'done', code: CODE, ok: true }]);
    });
    expect(daemon.requests('POST /api/v1/auth/step-up')).toHaveLength(1);
  });

  it('stays silent about a handed-over code another tab may have taken first', async () => {
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    fakeApi({
      'POST /api/v1/auth/step-up': () => problem(401, 'step-up-code-invalid', 'used'),
    });
    show('');
    mount();
    const [channel] = FakeChannel.all;
    act(() => {
      channel?.hear({ type: 'code', code: CODE });
    });
    await waitFor(() => {
      expect(channel?.sent).toEqual([{ type: 'done', code: CODE, ok: false }]);
    });
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('ignores messages that are not about codes', () => {
    window.sessionStorage.setItem('nullradix-tab-key', KEY);
    const daemon = fakeApi();
    show('');
    mount();
    const [channel] = FakeChannel.all;
    act(() => {
      channel?.hear('code');
      channel?.hear({ type: 'code' });
      channel?.hear({ type: 'other', code: CODE });
    });
    expect(daemon.requests('POST /api/v1/auth/step-up')).toHaveLength(0);
  });
});
