import { queryOptions, useMutation } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import { useInvalidate } from '../../shared/query/useInvalidate';
import { dispatcherKeys } from './queries';

export type DispatcherStatus = components['schemas']['DispatcherStatusDto'];
export type DispatcherPhase = components['schemas']['DispatcherPhaseDto'];
export type PassReport = components['schemas']['PassReportDto'];

export const PAUSE_MINUTES = 60;

export function dispatcherStatusQuery() {
  return queryOptions({
    queryKey: dispatcherKeys.status(),
    queryFn: async () => unwrap(await api.GET('/api/v1/dispatcher')),
  });
}

export function useSetDispatcherEnabled() {
  const invalidate = useInvalidate(dispatcherKeys.status());
  return useMutation({
    mutationFn: async (enabled: boolean) =>
      unwrap(await api.PATCH('/api/v1/dispatcher', { body: { enabled } })),
    onSettled: invalidate,
  });
}

export function usePauseDispatcher() {
  const invalidate = useInvalidate(dispatcherKeys.status());
  return useMutation({
    mutationFn: async () =>
      unwrap(await api.POST('/api/v1/dispatcher/pause', { body: { minutes: PAUSE_MINUTES } })),
    onSettled: invalidate,
  });
}

export function useResumeDispatcher() {
  const invalidate = useInvalidate(dispatcherKeys.status());
  return useMutation({
    mutationFn: async () => unwrap(await api.POST('/api/v1/dispatcher/resume')),
    onSettled: invalidate,
  });
}

export function useRunDispatcherPass() {
  const invalidate = useInvalidate(dispatcherKeys.status());
  return useMutation({
    mutationFn: async () => unwrap(await api.POST('/api/v1/dispatcher/passes')),
    onSettled: invalidate,
  });
}
