import { MutationCache, QueryCache, QueryClient, type DefaultOptions } from '@tanstack/react-query';
import { recheckSession } from '../features/session/queries';
import { ApiProblemError, isUnauthorized } from '../shared/api/errors';

export const MAX_RETRIES = 2;

export function shouldRetry(failureCount: number, error: unknown): boolean {
  if (error instanceof ApiProblemError && error.status < 500) return false;
  return failureCount < MAX_RETRIES;
}

export function createQueryClient(queries: DefaultOptions['queries'] = {}): QueryClient {
  // A 401 anywhere means the session ended; recheck it so the session gate shows.
  const onUnauthorized = (error: unknown) => {
    if (isUnauthorized(error)) void recheckSession(client);
  };
  const client: QueryClient = new QueryClient({
    queryCache: new QueryCache({ onError: onUnauthorized }),
    mutationCache: new MutationCache({ onError: onUnauthorized }),
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
