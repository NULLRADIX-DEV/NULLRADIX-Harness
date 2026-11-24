import createClient from 'openapi-fetch';
import type { paths } from './schema';

export type { components, paths } from './schema';

// Major API version this build speaks; equals info.version in openapi.json.
export const EXPECTED_API_VERSION = 1;

// Resolves fetch per call so tests can stub it.
export const api = createClient<paths>({
  baseUrl: window.location.origin,
  fetch: (request) => globalThis.fetch(request),
});
