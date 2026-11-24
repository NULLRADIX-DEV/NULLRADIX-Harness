import { queryOptions } from '@tanstack/react-query';
import { api } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';

export const HEALTH_REFRESH_MS = 10_000;

export const healthKeys = {
  all: ['daemon', 'health'] as const,
};

export function healthQuery() {
  return queryOptions({
    queryKey: healthKeys.all,
    queryFn: async () => unwrap(await api.GET('/api/v1/health')),
    refetchInterval: HEALTH_REFRESH_MS,
    // Polling already retries; a down daemon should show up at once.
    retry: false,
  });
}
