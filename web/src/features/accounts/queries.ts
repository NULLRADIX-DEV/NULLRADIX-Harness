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

export type AccountStatus = components['schemas']['AccountStatusDto'];

export const USAGE_REFRESH_MS = 120_000;
export const USAGE_RETRY_MS = 15_000;
export const LOGIN_POLL_MS = 2_000;

// Short-lived failures are asked again soon, readings every two minutes.
export function usageQuery() {
  return queryOptions({
    queryKey: [...accountKeys.all, 'usage'] as const,
    queryFn: async () => unwrap(await api.GET('/api/v1/accounts/usage')).items,
    refetchInterval: (query) => {
      const statuses = query.state.data ?? [];
      if (statuses.some((status) => status.login?.state === 'running')) return LOGIN_POLL_MS;
      const transient = statuses.some((status) => {
        const reason = status.usage.reason;
        return reason === 'request-failed' || reason === 'token-expired';
      });
      return transient ? USAGE_RETRY_MS : USAGE_REFRESH_MS;
    },
  });
}

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

export type LoginStatus = components['schemas']['LoginDto'];

export function useStartLogin() {
  const invalidate = useInvalidate(accountKeys.all);
  return useMutation({
    mutationFn: async ({ id, email }: { id: string; email?: string }) =>
      unwrap(
        await api.POST('/api/v1/accounts/{accountId}/login', {
          params: { path: { accountId: id } },
          body: email ? { email } : {},
        }),
      ),
    onSettled: invalidate,
  });
}

export function useCreateAccountLogin() {
  const invalidate = useInvalidate(accountKeys.all);
  return useMutation({
    mutationFn: async (input: components['schemas']['CreateAccountLogin']) =>
      unwrap(await api.POST('/api/v1/accounts/login', { body: input })),
    onSettled: invalidate,
  });
}

// The code is one-time and only travels to the daemon; nothing keeps it.
export function useSendLoginCode() {
  const invalidate = useInvalidate(accountKeys.all);
  return useMutation({
    mutationFn: async ({ id, code }: { id: string; code: string }) => {
      unwrapEmpty(
        await api.POST('/api/v1/accounts/{accountId}/login/code', {
          params: { path: { accountId: id } },
          body: { code },
        }),
      );
    },
    onSettled: invalidate,
  });
}

export function useCancelLogin() {
  const invalidate = useInvalidate(accountKeys.all);
  return useMutation({
    mutationFn: async (id: string) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/accounts/{accountId}/login', {
          params: { path: { accountId: id } },
        }),
      );
    },
    onSettled: invalidate,
  });
}
