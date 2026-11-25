import { queryOptions, useMutation } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';

export type DispatcherConfig = components['schemas']['DispatcherConfigDto'];
export type DispatcherConfigView = components['schemas']['DispatcherConfigView'];
export type ProfileDto = components['schemas']['ProfileDto'];
export type RuleDto = components['schemas']['RuleDto'];

export const dispatcherKeys = {
  all: ['dispatcher'] as const,
  config: () => [...dispatcherKeys.all, 'config'] as const,
};

export const dispatcherLive: ChangeRegistry = {
  'dispatcher-config': [dispatcherKeys.all],
  account: [dispatcherKeys.all],
};

export function dispatcherConfigQuery() {
  return queryOptions({
    queryKey: dispatcherKeys.config(),
    queryFn: async () => unwrap(await api.GET('/api/v1/dispatcher/config')),
  });
}

export function useSaveDispatcherConfig() {
  const invalidate = useInvalidate(dispatcherKeys.all);
  return useMutation({
    mutationFn: async (input: { version: number; config: DispatcherConfig }) =>
      unwrap(await api.PUT('/api/v1/dispatcher/config', { body: input })),
    onSettled: invalidate,
  });
}
