import { queryOptions, useMutation } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';

export type DispatcherConfig = components['schemas']['DispatcherConfigDto'];
export type DispatcherConfigView = components['schemas']['DispatcherConfigView'];
export type ProfileDto = components['schemas']['ProfileDto'];
export type RuleDto = components['schemas']['RuleDto'];
export type PromptDto = components['schemas']['PromptDto'];
export type PromptKind = components['schemas']['PromptKindDto'];
export type PromptList = components['schemas']['PromptListDto'];
export type PromptPreview = components['schemas']['PromptPreviewDto'];

export const dispatcherKeys = {
  all: ['dispatcher'] as const,
  config: () => [...dispatcherKeys.all, 'config'] as const,
  status: () => [...dispatcherKeys.all, 'status'] as const,
  prompts: () => [...dispatcherKeys.all, 'prompts'] as const,
  stats: () => [...dispatcherKeys.all, 'stats'] as const,
};

export const dispatcherLive: ChangeRegistry = {
  'dispatcher-config': [dispatcherKeys.all],
  account: [dispatcherKeys.all],
  'dispatcher-state': [dispatcherKeys.status(), dispatcherKeys.stats()],
  run: [dispatcherKeys.status(), dispatcherKeys.stats()],
  ticket: [dispatcherKeys.status(), dispatcherKeys.stats()],
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

export function promptsQuery() {
  return queryOptions({
    queryKey: dispatcherKeys.prompts(),
    queryFn: async () => unwrap(await api.GET('/api/v1/dispatcher/prompts')),
  });
}

export function useSavePrompt() {
  const invalidate = useInvalidate(dispatcherKeys.all);
  return useMutation({
    mutationFn: async (input: { kind: PromptKind; template: string; version: number }) =>
      unwrap(
        await api.PUT('/api/v1/dispatcher/prompts/{kind}', {
          params: { path: { kind: input.kind } },
          body: { template: input.template, version: input.version },
        }),
      ),
    onSettled: invalidate,
  });
}

export function useResetPrompt() {
  const invalidate = useInvalidate(dispatcherKeys.all);
  return useMutation({
    mutationFn: async (input: { kind: PromptKind; version: number }) =>
      unwrap(
        await api.DELETE('/api/v1/dispatcher/prompts/{kind}', {
          params: { path: { kind: input.kind }, query: { version: input.version } },
        }),
      ),
    onSettled: invalidate,
  });
}

export function usePreviewPrompt() {
  return useMutation({
    mutationFn: async (input: { kind: PromptKind; key: string; template: string }) =>
      unwrap(
        await api.POST('/api/v1/dispatcher/prompt-previews', {
          body: {
            kind: input.kind,
            key: input.key.trim() === '' ? null : input.key.trim(),
            template: input.template,
          },
        }),
      ),
  });
}
