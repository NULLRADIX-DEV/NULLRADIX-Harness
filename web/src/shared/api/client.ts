import createClient from 'openapi-fetch';
import type { paths } from './schema';

export type { components, paths } from './schema';

// Resolves fetch per call so tests can stub it.
export const api = createClient<paths>({
  baseUrl: window.location.origin,
  fetch: (request) => globalThis.fetch(request),
});
