import { cleanup, render, renderHook, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ThemeHostProvider, themeAssetUrl, useThemeAsset } from '../src/index.js';
import type { ThemeAssetError, ThemeAssetState, ThemeHost } from '../src/index.js';

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

// Host of which only the theme id counts
function themeHost(themeId: string): ThemeHost {
  return { colorScheme: { scheme: 'dark', themeId, mode: 'dark' } } as ThemeHost;
}

const wrapper =
  (themeId: string) =>
  ({ children }: { children?: ReactNode }) => (
    <ThemeHostProvider value={themeHost(themeId)}>{children}</ThemeHostProvider>
  );

// Proxy reply with a JSON error
const jsonError = (status: number, code: string, message: string) =>
  new Response(JSON.stringify({ code, message }), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });

// A request that stays open until its signal aborts it
function pending(signalSink: AbortSignal[]) {
  return vi.fn((_url: string | URL | Request, init?: RequestInit) => {
    const signal = init?.signal;
    if (!signal) return Promise.reject(new Error('no signal'));
    signalSink.push(signal);
    return new Promise<Response>((_, reject) => {
      signal.addEventListener('abort', () => {
        reject(new DOMException('aborted', 'AbortError'));
      });
    });
  });
}

const okResponse = () => Promise.resolve(new Response('bytes', { status: 200 }));

describe('themeAssetUrl', () => {
  it('builds the proxy URL with an encoded address', () => {
    expect(themeAssetUrl('minecraft', 'https://a.example.com/x.png')).toBe(
      '/api/themes/minecraft/fetch?url=https%3A%2F%2Fa.example.com%2Fx.png',
    );
  });

  it('appends sha256 and entry encoded', () => {
    const url = themeAssetUrl('minecraft', 'https://a.example.com/c.jar', {
      sha256: 'ab12',
      entry: 'assets/minecraft/textures/block/dirt.png',
    });
    expect(url).toBe(
      '/api/themes/minecraft/fetch?url=https%3A%2F%2Fa.example.com%2Fc.jar&sha256=ab12&entry=assets%2Fminecraft%2Ftextures%2Fblock%2Fdirt.png',
    );
  });
});

describe('themeAssetUrl in the desktop app', () => {
  // The app puts server address and start token into the window; its page has another origin
  beforeEach(() => {
    Object.assign(window, { __NULLRADIX__: { base: 'http://127.0.0.1:4810', token: 'secret' } });
    vi.stubGlobal('fetch', vi.fn(okResponse));
  });
  afterEach(() => {
    delete (window as { __NULLRADIX__?: unknown }).__NULLRADIX__;
  });

  it('points at the local server and identifies itself with the start token', () => {
    const url = themeAssetUrl('minecraft', 'https://a.example.com/c.jar', {
      sha256: 'ab12',
      entry: 'a/b.png',
    });
    expect(url).toBe(
      'http://127.0.0.1:4810/api/themes/minecraft/fetch?url=https%3A%2F%2Fa.example.com%2Fc.jar&sha256=ab12&entry=a%2Fb.png&token=secret',
    );
  });

  it('keeps the relative address without a token', () => {
    Object.assign(window, { __NULLRADIX__: { base: 'http://127.0.0.1:4810', token: '' } });
    expect(themeAssetUrl('minecraft', 'https://a.example.com/x.png')).toBe(
      '/api/themes/minecraft/fetch?url=https%3A%2F%2Fa.example.com%2Fx.png',
    );
  });

  it('useThemeAsset loads through the same address and returns it', async () => {
    const { result } = renderHook(() => useThemeAsset('https://a.example.com/x.png'), {
      wrapper: wrapper('demo-theme'),
    });
    const proxy =
      'http://127.0.0.1:4810/api/themes/demo-theme/fetch?url=https%3A%2F%2Fa.example.com%2Fx.png&token=secret';
    await waitFor(() => {
      expect(result.current).toEqual({ status: 'ready', url: proxy, error: null });
    });
    expect(vi.mocked(fetch).mock.calls[0]?.[0]).toBe(proxy);
  });
});

describe('useThemeAsset', () => {
  let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;
  beforeEach(() => {
    fetchMock = vi.fn<typeof fetch>(okResponse);
    vi.stubGlobal('fetch', fetchMock);
  });

  it('reports loading and then ready with the proxy URL of the active theme', async () => {
    const { result } = renderHook(() => useThemeAsset('https://a.example.com/x.png'), {
      wrapper: wrapper('demo-theme'),
    });
    expect(result.current).toEqual({ status: 'loading', url: null, error: null });
    const proxy = '/api/themes/demo-theme/fetch?url=https%3A%2F%2Fa.example.com%2Fx.png';
    await waitFor(() => {
      expect(result.current).toEqual({ status: 'ready', url: proxy, error: null });
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]?.[0]).toBe(proxy);
  });

  it('reports the error code of the proxy with the HTTP status', async () => {
    fetchMock.mockResolvedValue(jsonError(403, 'theme_fetch_forbidden', 'Domain not allowed'));
    const { result } = renderHook(() => useThemeAsset('https://other.example.com/x.png'), {
      wrapper: wrapper('demo-theme'),
    });
    await waitFor(() => {
      expect(result.current.status).toBe('error');
    });
    expect(result.current.url).toBeNull();
    expect(result.current.error).toEqual<ThemeAssetError>({
      status: 403,
      code: 'theme_fetch_forbidden',
      message: 'Domain not allowed',
    });
  });

  it('falls back to http_<status> without a JSON body', async () => {
    fetchMock.mockResolvedValue(new Response('<html>', { status: 502 }));
    const { result } = renderHook(() => useThemeAsset('https://a.example.com/x.png'), {
      wrapper: wrapper('demo-theme'),
    });
    await waitFor(() => {
      expect(result.current.status).toBe('error');
    });
    expect(result.current.error?.status).toBe(502);
    expect(result.current.error?.code).toBe('http_502');
  });

  it('reports a network error as code network', async () => {
    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
    const { result } = renderHook(() => useThemeAsset('https://a.example.com/x.png'), {
      wrapper: wrapper('demo-theme'),
    });
    await waitFor(() => {
      expect(result.current.status).toBe('error');
    });
    expect(result.current.error).toMatchObject({ status: null, code: 'network' });
    expect(result.current.url).toBeNull();
  });

  it('uses opts.themeId instead of the host id and passes sha256 and entry on', async () => {
    const options = { themeId: 'other', sha256: 'ff', entry: 'a/b.png' };
    const { result } = renderHook(() => useThemeAsset('https://a.example.com/c.jar', options), {
      wrapper: wrapper('demo-theme'),
    });
    const proxy =
      '/api/themes/other/fetch?url=https%3A%2F%2Fa.example.com%2Fc.jar&sha256=ff&entry=a%2Fb.png';
    await waitFor(() => {
      expect(result.current.url).toBe(proxy);
    });
    expect(fetchMock.mock.calls[0]?.[0]).toBe(proxy);
  });

  it.each([[null], [undefined], ['']])(
    'is idle for an empty URL (%s) and does not fetch',
    (url) => {
      const { result } = renderHook(() => useThemeAsset(url), { wrapper: wrapper('demo-theme') });
      expect(result.current).toEqual({ status: 'idle', url: null, error: null });
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  it('reloads on a URL change and aborts the old request', async () => {
    const signals: AbortSignal[] = [];
    fetchMock.mockImplementationOnce(pending(signals));
    const { result, rerender } = renderHook(({ url }) => useThemeAsset(url), {
      wrapper: wrapper('demo-theme'),
      initialProps: { url: 'https://a.example.com/1.png' },
    });
    expect(result.current.status).toBe('loading');
    rerender({ url: 'https://a.example.com/2.png' });
    expect(signals[0]?.aborted).toBe(true);
    expect(result.current.status).toBe('loading');
    const proxy = '/api/themes/demo-theme/fetch?url=https%3A%2F%2Fa.example.com%2F2.png';
    await waitFor(() => {
      expect(result.current).toEqual({ status: 'ready', url: proxy, error: null });
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('is idle again after a change to an empty URL and aborts', () => {
    const signals: AbortSignal[] = [];
    fetchMock.mockImplementationOnce(pending(signals));
    const { result, rerender } = renderHook<ThemeAssetState, { url: string | null }>(
      ({ url }) => useThemeAsset(url),
      {
        wrapper: wrapper('demo-theme'),
        initialProps: { url: 'https://a.example.com/1.png' },
      },
    );
    rerender({ url: null });
    expect(signals[0]?.aborted).toBe(true);
    expect(result.current).toEqual({ status: 'idle', url: null, error: null });
  });

  it('aborts the request on unmount', () => {
    const signals: AbortSignal[] = [];
    fetchMock.mockImplementationOnce(pending(signals));
    const { unmount } = renderHook(() => useThemeAsset('https://a.example.com/x.png'), {
      wrapper: wrapper('demo-theme'),
    });
    unmount();
    expect(signals[0]?.aborted).toBe(true);
  });

  it('runs with opts.themeId even without a host provider', async () => {
    const { result } = renderHook(() =>
      useThemeAsset('https://a.example.com/x.png', { themeId: 'solo' }),
    );
    await waitFor(() => {
      expect(result.current.status).toBe('ready');
    });
    expect(result.current.url).toBe(
      '/api/themes/solo/fetch?url=https%3A%2F%2Fa.example.com%2Fx.png',
    );
  });

  it('throws a clear error without a host provider and without opts.themeId', () => {
    function Probe() {
      useThemeAsset('https://a.example.com/x.png');
      return null;
    }
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    expect(() => render(<Probe />)).toThrow(
      /useThemeAsset needs the ThemeHostProvider of the NULLRADIX host or opts\.themeId/,
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('works in a component as an image with loading and error display', async () => {
    function Picture() {
      const asset: ThemeAssetState = useThemeAsset('https://a.example.com/x.png');
      if (asset.status === 'loading') return <span>loading</span>;
      if (asset.status === 'error') return <span>{asset.error?.code}</span>;
      return <img alt="x" src={asset.url ?? undefined} />;
    }
    render(<Picture />, { wrapper: wrapper('demo-theme') });
    expect(screen.getByText('loading')).toBeTruthy();
    const img = await screen.findByRole('img');
    expect(img.getAttribute('src')).toBe(
      '/api/themes/demo-theme/fetch?url=https%3A%2F%2Fa.example.com%2Fx.png',
    );
  });
});
