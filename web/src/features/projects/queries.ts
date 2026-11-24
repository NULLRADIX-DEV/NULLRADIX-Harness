import { queryOptions, useMutation } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap, unwrapEmpty } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';

export type Project = components['schemas']['ProjectDto'];
export type CreateProjectInput = components['schemas']['CreateProject'];

export interface RenameProjectInput {
  id: string;
  name: string;
  version: number;
}

export interface DeleteProjectInput {
  id: string;
  version: number;
}

export const projectKeys = {
  all: ['projects'] as const,
  list: () => [...projectKeys.all, 'list'] as const,
};

export const projectsLive: ChangeRegistry = {
  project: [projectKeys.all],
};

export function projectsQuery() {
  return queryOptions({
    queryKey: projectKeys.list(),
    queryFn: async () => unwrap(await api.GET('/api/v1/projects')).items,
  });
}

export function useCreateProject() {
  const invalidate = useInvalidate(projectKeys.all);
  return useMutation({
    mutationFn: async (input: CreateProjectInput) =>
      unwrap(await api.POST('/api/v1/projects', { body: input })),
    onSettled: invalidate,
  });
}

export function useRenameProject() {
  const invalidate = useInvalidate(projectKeys.all);
  return useMutation({
    mutationFn: async ({ id, name, version }: RenameProjectInput) =>
      unwrap(
        await api.PATCH('/api/v1/projects/{projectId}', {
          params: { path: { projectId: id } },
          body: { name, version },
        }),
      ),
    onSettled: invalidate,
  });
}

export function useDeleteProject() {
  const invalidate = useInvalidate(projectKeys.all);
  return useMutation({
    mutationFn: async ({ id, version }: DeleteProjectInput) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/projects/{projectId}', {
          params: { path: { projectId: id }, query: { version } },
        }),
      );
    },
    onSettled: invalidate,
  });
}
