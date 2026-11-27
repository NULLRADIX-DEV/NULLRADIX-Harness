import { queryOptions, useMutation, useQueryClient } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { sessionKeys } from '../session/queries';

export type StepUpSetting = components['schemas']['StepUpSetting'];

export const stepUpKeys = {
  all: ['step-up'] as const,
  setting: () => [...stepUpKeys.all, 'setting'] as const,
};

// The switch is a setting of the daemon, so its changes arrive as setting changes.
export const stepUpLive: ChangeRegistry = {
  setting: [stepUpKeys.all],
};

export function stepUpSettingQuery() {
  return queryOptions({
    queryKey: stepUpKeys.setting(),
    queryFn: async () => unwrap(await api.GET('/api/v1/settings/step-up')),
  });
}

export function useSetStepUp() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (required: boolean) =>
      unwrap(await api.PUT('/api/v1/settings/step-up', { body: { required } })),
    onSettled: () => queryClient.invalidateQueries({ queryKey: stepUpKeys.all }),
  });
}

// Gives the daemon the code a terminal printed; the session is then confirmed for a while.
export function useRedeemStepUp() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (code: string) =>
      unwrap(await api.POST('/api/v1/auth/step-up', { body: { code } })),
    onSettled: () => queryClient.invalidateQueries({ queryKey: sessionKeys.all }),
  });
}
