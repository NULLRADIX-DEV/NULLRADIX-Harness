import {
  MutationCache,
  QueryCache,
  QueryClient,
  hashKey,
  type DefaultOptions,
} from '@tanstack/react-query';
import { sessionKeys } from '../features/session/queries';
import { ApiProblemError, isUnauthorized } from '../shared/api/errors';

export const MAX_RETRIES = 2;

export function shouldRetry(failureCount: number, error: unknown): boolean {
  if (error instanceof ApiProblemError && error.status < 500) return false;
  return failureCount < MAX_RETRIES;
}

export function createQueryClient(queries: DefaultOptions['queries'] = {}): QueryClient {
  // A 401 anywhere means the session ended; recheck it so the session gate shows.
  const recheckSession = (error: unknown) => {
    if (!isUnauthorized(error)) return;
    const filters = { queryKey: sessionKeys.all };
    // cancel first, else an in-flight check swallows the refetch
    void client.cancelQueries(filters).then(() => client.invalidateQueries(filters));
  };
  const client: QueryClient = new QueryClient({
    queryCache: new QueryCache({
      onError: (error, query) => {
        if (query.queryHash !== hashKey(sessionKeys.all)) recheckSession(error);
      },
    }),
    mutationCache: new MutationCache({ onError: recheckSession }),
    defaultOptions: {
      queries: {
        staleTime: 5_000,
        refetchOnWindowFocus: false,
        retry: shouldRetry,
        ...queries,
      },
    },
  });
  return client;
}
