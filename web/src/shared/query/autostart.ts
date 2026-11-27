import { queryOptions, useMutation, useQueryClient } from '@tanstack/react-query';
import { api, type components } from '../api/client';
import { unwrap } from '../api/errors';

export type Autostart = components['schemas']['AutostartDto'];

export const autostartKeys = {
  all: ['autostart'] as const,
};

export function autostartQuery() {
  return queryOptions({
    queryKey: autostartKeys.all,
    queryFn: async () => unwrap(await api.GET('/api/v1/settings/autostart')),
  });
}

export function useSetAutostart() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (enabled: boolean) =>
      unwrap(await api.PUT('/api/v1/settings/autostart', { body: { enabled } })),
    onSuccess: (status) => {
      queryClient.setQueryData(autostartKeys.all, status);
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: autostartKeys.all }),
  });
}
