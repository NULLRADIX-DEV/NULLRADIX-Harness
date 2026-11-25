import { queryOptions, useMutation } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap, unwrapEmpty } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';

export type Account = components['schemas']['AccountDto'];
export type AccountList = components['schemas']['AccountList'];
export type CreateAccountInput = components['schemas']['CreateAccount'];
export type UpdateAccountInput = components['schemas']['UpdateAccount'];

export const accountKeys = {
  all: ['accounts'] as const,
  list: () => [...accountKeys.all, 'list'] as const,
};

export const accountsLive: ChangeRegistry = {
  account: [accountKeys.all],
};

export function accountsQuery() {
  return queryOptions({
    queryKey: accountKeys.list(),
    queryFn: async () => unwrap(await api.GET('/api/v1/accounts')),
  });
}

export function useCreateAccount() {
  const invalidate = useInvalidate(accountKeys.all);
  return useMutation({
    mutationFn: async (input: CreateAccountInput) =>
      unwrap(await api.POST('/api/v1/accounts', { body: input })),
    onSettled: invalidate,
  });
}

export function useUpdateAccount() {
  const invalidate = useInvalidate(accountKeys.all);
  return useMutation({
    mutationFn: async ({ id, ...body }: UpdateAccountInput & { id: string }) =>
      unwrap(
        await api.PATCH('/api/v1/accounts/{accountId}', {
          params: { path: { accountId: id } },
          body,
        }),
      ),
    onSettled: invalidate,
  });
}

export function useDeleteAccount() {
  const invalidate = useInvalidate(accountKeys.all);
  return useMutation({
    mutationFn: async ({
      id,
      version,
      deleteDir,
    }: {
      id: string;
      version: number;
      deleteDir: boolean;
    }) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/accounts/{accountId}', {
          params: { path: { accountId: id }, query: { version, deleteDir } },
        }),
      );
    },
    onSettled: invalidate,
  });
}

// The token is sent once and never kept: the account only says that one is set.
export function useSetRunToken() {
  const invalidate = useInvalidate(accountKeys.all);
  return useMutation({
    mutationFn: async ({ id, token, version }: { id: string; token: string; version: number }) =>
      unwrap(
        await api.PUT('/api/v1/accounts/{accountId}/run-token', {
          params: { path: { accountId: id } },
          body: { token, version },
        }),
      ),
    onSettled: invalidate,
  });
}

export function useClearRunToken() {
  const invalidate = useInvalidate(accountKeys.all);
  return useMutation({
    mutationFn: async ({ id, version }: { id: string; version: number }) =>
      unwrap(
        await api.DELETE('/api/v1/accounts/{accountId}/run-token', {
          params: { path: { accountId: id }, query: { version } },
        }),
      ),
    onSettled: invalidate,
  });
}
