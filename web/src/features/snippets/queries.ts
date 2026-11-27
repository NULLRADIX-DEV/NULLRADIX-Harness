import { queryOptions, useMutation } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap, unwrapEmpty } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';

export type Snippet = components['schemas']['SnippetDto'];
export type CreateSnippetInput = components['schemas']['CreateSnippet'];

export interface UpdateSnippetInput {
  id: string;
  version: number;
  name?: string;
  css?: string;
  enabled?: boolean;
}

export interface DeleteSnippetInput {
  id: string;
  version: number;
}

export const snippetKeys = {
  all: ['snippets'] as const,
  list: () => [...snippetKeys.all, 'list'] as const,
};

export const snippetsLive: ChangeRegistry = {
  snippet: [snippetKeys.all],
};

export function snippetsQuery() {
  return queryOptions({
    queryKey: snippetKeys.list(),
    queryFn: async () => unwrap(await api.GET('/api/v1/snippets')).items,
  });
}

export function useCreateSnippet() {
  const invalidate = useInvalidate(snippetKeys.all);
  return useMutation({
    mutationFn: async (input: CreateSnippetInput) =>
      unwrap(await api.POST('/api/v1/snippets', { body: input })),
    onSettled: invalidate,
  });
}

export function useUpdateSnippet() {
  const invalidate = useInvalidate(snippetKeys.all);
  return useMutation({
    mutationFn: async ({ id, version, name, css, enabled }: UpdateSnippetInput) =>
      unwrap(
        await api.PATCH('/api/v1/snippets/{id}', {
          params: { path: { id } },
          body: {
            version,
            ...(name !== undefined && { name }),
            ...(css !== undefined && { css }),
            ...(enabled !== undefined && { enabled }),
          },
        }),
      ),
    onSettled: invalidate,
  });
}

export function useDeleteSnippet() {
  const invalidate = useInvalidate(snippetKeys.all);
  return useMutation({
    mutationFn: async ({ id, version }: DeleteSnippetInput) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/snippets/{id}', {
          params: { path: { id }, query: { version } },
        }),
      );
    },
    onSettled: invalidate,
  });
}
