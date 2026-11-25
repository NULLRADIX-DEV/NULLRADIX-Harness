import { queryOptions, useMutation, useQueryClient } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap, unwrapEmpty } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { noteKeys } from '../notes/queries';
import { ticketKeys } from '../tickets/queries';

export type InboxItem = components['schemas']['InboxItemDto'];
export type InboxKind = components['schemas']['InboxKindDto'];
export type InboxList = components['schemas']['InboxList'];

export const INBOX_KINDS: readonly InboxKind[] = [
  'merge',
  'setup',
  'question',
  'accept',
  'blocked',
  'planning',
  'note',
];

export const KIND_LABELS: Record<InboxKind, string> = {
  merge: 'Merge failed',
  setup: 'Setup failed',
  question: 'Question',
  accept: 'Acceptance',
  blocked: 'Blocked',
  planning: 'In planning',
  note: 'Note',
};

export const inboxKeys = {
  all: ['inbox'] as const,
  list: (boardId: string | null) => [...inboxKeys.all, 'list', boardId ?? 'all'] as const,
};

// Tickets, questions and notes make up the inbox, so any of their changes refetches it.
export const inboxLive: ChangeRegistry = {
  ticket: [inboxKeys.all],
  question: [inboxKeys.all],
  note: [inboxKeys.all],
  board: [inboxKeys.all],
};

export function inboxQuery(boardId: string | null) {
  return queryOptions({
    queryKey: inboxKeys.list(boardId),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/me/inbox', {
          params: { query: boardId === null ? {} : { boardId } },
        }),
      ),
  });
}

// Moves one entry; a new list, or the same one when nothing moves.
export function moveItem<T>(items: readonly T[], from: number, to: number): readonly T[] {
  if (from === to || from < 0 || to < 0 || from >= items.length || to >= items.length) {
    return items;
  }
  const next = [...items];
  const [moved] = next.splice(from, 1);
  if (moved !== undefined) next.splice(to, 0, moved);
  return next;
}

// Shows the new order at once and restores the old one when saving fails.
export function useSaveOrder(boardId: string | null) {
  const queryClient = useQueryClient();
  const key = inboxKeys.list(boardId);
  return useMutation({
    mutationFn: async (items: readonly InboxItem[]) =>
      unwrap(
        await api.PUT('/api/v1/me/inbox/order', {
          body: { items: items.map((item) => ({ type: item.type, key: item.key })) },
        }),
      ),
    onMutate: async (items) => {
      await queryClient.cancelQueries({ queryKey: key });
      const before = queryClient.getQueryData<InboxList>(key);
      if (before) queryClient.setQueryData<InboxList>(key, { ...before, items: [...items] });
      return { before };
    },
    onError: (_error, _items, context) => {
      if (context?.before) queryClient.setQueryData(key, context.before);
    },
  });
}

export function useResetOrder() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      unwrapEmpty(await api.DELETE('/api/v1/me/inbox/order'));
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: inboxKeys.all }),
  });
}

// Accepting a ticket or promoting a note takes the entry out of the inbox.
export function useInboxAction() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (item: InboxItem) => {
      if (item.type === 'note') {
        unwrap(
          await api.POST('/api/v1/notes/{noteNumber}/promote', {
            params: { path: { noteNumber: item.key } },
            body: {},
          }),
        );
      } else {
        unwrap(
          await api.POST('/api/v1/tickets/{ticketKey}/accept', {
            params: { path: { ticketKey: item.key } },
            body: {},
          }),
        );
      }
    },
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: inboxKeys.all }),
        queryClient.invalidateQueries({ queryKey: ticketKeys.all }),
        queryClient.invalidateQueries({ queryKey: noteKeys.all }),
      ]),
  });
}
