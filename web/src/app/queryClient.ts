import { QueryClient, type DefaultOptions } from '@tanstack/react-query';
import { ApiProblemError } from '../shared/api/errors';

export const MAX_RETRIES = 2;

export function shouldRetry(failureCount: number, error: unknown): boolean {
  if (error instanceof ApiProblemError && error.status < 500) return false;
  return failureCount < MAX_RETRIES;
}

export function createQueryClient(queries: DefaultOptions['queries'] = {}): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 5_000,
        refetchOnWindowFocus: false,
        retry: shouldRetry,
        ...queries,
      },
    },
  });
}
