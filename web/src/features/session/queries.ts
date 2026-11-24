import { queryOptions, type QueryClient } from '@tanstack/react-query';
import { api } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';

export const SESSION_RECHECK_MS = 5_000;

export const sessionKeys = {
  all: ['auth', 'me'] as const,
};

export function sessionQuery() {
  return queryOptions({
    queryKey: sessionKeys.all,
    queryFn: async () => unwrap(await api.GET('/api/v1/auth/me')),
    // Another tab may finish `nullradix open`, or the daemon may come back.
    refetchInterval: (query) => (query.state.error ? SESSION_RECHECK_MS : false),
  });
}

export async function recheckSession(client: QueryClient): Promise<void> {
  const filters = { queryKey: sessionKeys.all };
  // cancel first, else an in-flight check swallows the refetch
  await client.cancelQueries(filters);
  await client.invalidateQueries(filters);
}
