import { queryOptions, useMutation } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap, unwrapEmpty } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';

export type Customization = components['schemas']['CustomizationDto'];
export type ExtensionPoint = components['schemas']['ExtensionPointDto'];
export type CreateCustomizationInput = components['schemas']['CreateCustomization'];

export interface UpdateCustomizationInput {
  point: ExtensionPoint;
  name: string;
  version: number;
  content?: string;
  enabled?: boolean;
}

export interface DeleteCustomizationInput {
  point: ExtensionPoint;
  name: string;
  version: number;
}

export const customizationKeys = {
  all: ['customizations'] as const,
  list: () => [...customizationKeys.all, 'list'] as const,
};

export const customizationsLive: ChangeRegistry = {
  customization: [customizationKeys.all],
};

export function customizationsQuery() {
  return queryOptions({
    queryKey: customizationKeys.list(),
    queryFn: async () => unwrap(await api.GET('/api/v1/customizations')).items,
  });
}

export function useCreateCustomization() {
  const invalidate = useInvalidate(customizationKeys.all);
  return useMutation({
    mutationFn: async (input: CreateCustomizationInput) =>
      unwrap(await api.POST('/api/v1/customizations', { body: input })),
    onSettled: invalidate,
  });
}

export function useUpdateCustomization() {
  const invalidate = useInvalidate(customizationKeys.all);
  return useMutation({
    mutationFn: async ({ point, name, version, content, enabled }: UpdateCustomizationInput) =>
      unwrap(
        await api.PATCH('/api/v1/customizations/{point}/{name}', {
          params: { path: { point, name } },
          body: {
            version,
            ...(content !== undefined && { content }),
            ...(enabled !== undefined && { enabled }),
          },
        }),
      ),
    onSettled: invalidate,
  });
}

export function useDeleteCustomization() {
  const invalidate = useInvalidate(customizationKeys.all);
  return useMutation({
    mutationFn: async ({ point, name, version }: DeleteCustomizationInput) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/customizations/{point}/{name}', {
          params: { path: { point, name }, query: { version } },
        }),
      );
    },
    onSettled: invalidate,
  });
}
