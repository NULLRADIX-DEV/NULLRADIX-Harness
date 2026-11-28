import createClient from 'openapi-fetch';
import type { paths } from './schema';
import { KEY_HEADER, readTabKey } from './tabKey';

export type { components, paths } from './schema';

// Major API version this build speaks; equals info.version in openapi.json.
export const EXPECTED_API_VERSION = 1;

// Resolves fetch per call so tests can stub it.
export const api = createClient<paths>({
  baseUrl: window.location.origin,
  fetch: (request) => globalThis.fetch(request),
});

// The cookie alone is no session: every call carries the key of this tab.
api.use({
  onRequest({ request }) {
    const key = readTabKey();
    if (key !== null) request.headers.set(KEY_HEADER, key);
    return request;
  },
});
