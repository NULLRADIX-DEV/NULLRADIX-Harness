import { queryOptions } from '@tanstack/react-query';
import { api } from '../../shared/api/client';
import { isUnauthorized, unwrap } from '../../shared/api/errors';

export const SESSION_RECHECK_MS = 5_000;

export const sessionKeys = {
  all: ['auth', 'me'] as const,
};

export function sessionQuery() {
  return queryOptions({
    queryKey: sessionKeys.all,
    queryFn: async () => unwrap(await api.GET('/api/v1/auth/me')),
    // Another tab may finish `nullradix open` while this one waits.
    refetchInterval: (query) => (isUnauthorized(query.state.error) ? SESSION_RECHECK_MS : false),
  });
}
