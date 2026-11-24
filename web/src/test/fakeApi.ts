import { vi } from 'vitest';

export type Handler = (request: Request) => Response | Promise<Response>;

export function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

export function problem(status: number, code: string, detail: string): Response {
  const body = { type: 'about:blank', title: 'Error', status, code, detail };
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/problem+json' },
  });
}

export const signedIn: Handler = () => json(200, { kind: 'session' });
export const signedOut: Handler = () => problem(401, 'credentials-missing', 'sign in first');

const defaults: Record<string, Handler> = {
  'GET /api/v1/health': () => json(200, { status: 'ok', version: '0.1.0', apiVersion: 1 }),
  'GET /api/v1/auth/me': signedIn,
};

// Routes by "METHOD /path"; unknown routes answer 404.
export function fakeApi(handlers: Record<string, Handler> = {}) {
  const routes = { ...defaults, ...handlers };
  const fetch = vi.fn((input: Request) => {
    const route = `${input.method} ${new URL(input.url).pathname}`;
    const handler = routes[route];
    return Promise.resolve(handler ? handler(input.clone()) : problem(404, 'not-found', route));
  });
  vi.stubGlobal('fetch', fetch);
  return {
    fetch,
    set(route: string, handler: Handler) {
      routes[route] = handler;
    },
    requests(route: string): Request[] {
      return fetch.mock.calls
        .map(([request]) => request)
        .filter((request) => `${request.method} ${new URL(request.url).pathname}` === route);
    },
  };
}
