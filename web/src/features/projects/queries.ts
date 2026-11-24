import { queryOptions, useMutation, useQueryClient } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap, unwrapEmpty } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';

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

function useInvalidateProjects() {
  const queryClient = useQueryClient();
  return () => queryClient.invalidateQueries({ queryKey: projectKeys.all });
}

export function useCreateProject() {
  const invalidate = useInvalidateProjects();
  return useMutation({
    mutationFn: async (input: CreateProjectInput) =>
      unwrap(await api.POST('/api/v1/projects', { body: input })),
    onSuccess: invalidate,
  });
}

export function useRenameProject() {
  const invalidate = useInvalidateProjects();
  return useMutation({
    mutationFn: async ({ id, name, version }: RenameProjectInput) =>
      unwrap(
        await api.PATCH('/api/v1/projects/{id}', {
          params: { path: { id } },
          body: { name, version },
        }),
      ),
    onSettled: invalidate,
  });
}

export function useDeleteProject() {
  const invalidate = useInvalidateProjects();
  return useMutation({
    mutationFn: async ({ id, version }: DeleteProjectInput) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/projects/{id}', {
          params: { path: { id, version } },
          // The spec lists version as a path parameter; the daemon reads it from the query.
          querySerializer: () => `version=${String(version)}`,
        }),
      );
    },
    onSettled: invalidate,
  });
}
