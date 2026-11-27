import { useEffect, useState } from 'react';
import { useOptionalHost } from './hooks.js';

/** Options of `useThemeAsset` and `themeAssetUrl` */
export interface ThemeAssetOptions {
  /** Theme whose domain grant and cache apply; default is the active theme of the host */
  themeId?: string;
  /** Expected sha256 checksum (hex) of the resource; on a mismatch the proxy answers 502 */
  sha256?: string;
  /** Path of an entry when the resource is a ZIP or JAR */
  entry?: string;
}

/** Error of the asset proxy: `code` from its JSON reply, `network` on a network error */
export interface ThemeAssetError {
  /** HTTP status of the reply; null on a network error */
  status: number | null;
  code: string;
  message: string;
}

/** State of a resource: `url` is set only when `ready`, `error` only when `error` */
export interface ThemeAssetState {
  status: 'idle' | 'loading' | 'ready' | 'error';
  url: string | null;
  error: ThemeAssetError | null;
}

const IDLE: ThemeAssetState = { status: 'idle', url: null, error: null };
const LOADING: ThemeAssetState = { status: 'loading', url: null, error: null };

interface AppValues {
  base?: string;
  token?: string;
}

// Server address and start token the desktop app puts into the window (`window.__NULLRADIX__`)
function appServer(): { base: string; token: string } | undefined {
  if (typeof window === 'undefined') return undefined;
  const values = (window as { __NULLRADIX__?: AppValues }).__NULLRADIX__;
  return values?.base && values.token ? { base: values.base, token: values.token } : undefined;
}

/** Local proxy URL of a foreign resource, such as for `url(...)` in CSS; checks nothing */
export function themeAssetUrl(
  themeId: string,
  url: string,
  opts: Omit<ThemeAssetOptions, 'themeId'> = {},
): string {
  const query = new URLSearchParams({ url });
  if (opts.sha256) query.set('sha256', opts.sha256);
  if (opts.entry) query.set('entry', opts.entry);
  const app = appServer();
  if (app) query.set('token', app.token);
  return `${app?.base ?? ''}/api/themes/${encodeURIComponent(themeId)}/fetch?${query.toString()}`;
}

// Reads the error from the proxy reply (`{ code, message }`), else `http_<status>`
async function readError(res: Response): Promise<ThemeAssetError> {
  try {
    const body = (await res.json()) as { code?: unknown; message?: unknown };
    if (typeof body.code === 'string') {
      const message = typeof body.message === 'string' ? body.message : res.statusText;
      return { status: res.status, code: body.code, message };
    }
  } catch {
    // not JSON: fall back below
  }
  return {
    status: res.status,
    code: `http_${String(res.status)}`,
    message: res.statusText || `HTTP ${String(res.status)}`,
  };
}

/** Loads a foreign resource through the server proxy of the active theme and returns its URL */
export function useThemeAsset(
  url: string | null | undefined,
  opts: ThemeAssetOptions = {},
): ThemeAssetState {
  const hostThemeId = useOptionalHost()?.colorScheme.themeId;
  const themeId = opts.themeId ?? hostThemeId;
  if (!themeId) {
    throw new Error(
      'useThemeAsset needs the ThemeHostProvider of the NULLRADIX host or opts.themeId - the component runs outside the NULLRADIX interface.',
    );
  }

  const proxyUrl = url ? themeAssetUrl(themeId, url, opts) : null;
  // A result belongs to exactly one proxy URL; after a change the state is `loading` again
  const [settled, setSettled] = useState<{ key: string; state: ThemeAssetState } | null>(null);

  useEffect(() => {
    if (!proxyUrl) return;
    const controller = new AbortController();
    const settle = (state: ThemeAssetState) => {
      if (!controller.signal.aborted) setSettled({ key: proxyUrl, state });
    };
    fetch(proxyUrl, { signal: controller.signal }).then(
      async (res) => {
        if (res.ok) {
          // The browser cache holds the bytes; the body is not needed here
          void res.body?.cancel().catch(() => undefined);
          settle({ status: 'ready', url: proxyUrl, error: null });
        } else {
          settle({ status: 'error', url: null, error: await readError(res) });
        }
      },
      (e: unknown) => {
        const message = e instanceof Error ? e.message : 'Network error';
        settle({ status: 'error', url: null, error: { status: null, code: 'network', message } });
      },
    );
    return () => {
      controller.abort();
    };
  }, [proxyUrl]);

  if (!proxyUrl) return IDLE;
  return settled?.key === proxyUrl ? settled.state : LOADING;
}
