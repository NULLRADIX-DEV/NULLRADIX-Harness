import { queryOptions, type QueryClient } from '@tanstack/react-query';
import { api } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';

export const SESSION_RECHECK_MS = 5_000;

export const sessionKeys = {
  all: ['auth', 'me'] as const,
};

// No session is `null`, not an error: a query without data turns pending on each refetch.
export function sessionQuery() {
  return queryOptions({
    queryKey: sessionKeys.all,
    queryFn: async () => {
      const result = await api.GET('/api/v1/auth/me');
      return result.response.status === 401 ? null : unwrap(result);
    },
    // Another tab may finish `nullradix open`, or the daemon may come back.
    refetchInterval: (query) =>
      query.state.data === null || query.state.error ? SESSION_RECHECK_MS : false,
  });
}

export async function recheckSession(client: QueryClient): Promise<void> {
  const filters = { queryKey: sessionKeys.all };
  // cancel first, else an in-flight check swallows the refetch
  await client.cancelQueries(filters);
  await client.invalidateQueries(filters);
}
