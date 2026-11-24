import { queryOptions, useMutation, useQueryClient } from '@tanstack/react-query';
import { useCallback } from 'react';
import { api, type components } from '../../shared/api/client';
import { unwrap, unwrapEmpty } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';
import { boardKeys } from '../boards/queries';

export type Ticket = components['schemas']['TicketDto'];
export type TicketDetail = components['schemas']['TicketDetailDto'];
export type Step = components['schemas']['StepDto'];
export type HistoryLine = components['schemas']['HistoryDto'];
export type Column = components['schemas']['ColumnDto'];
export type TicketStatus = components['schemas']['TicketStatusDto'];
export type Priority = components['schemas']['PriorityDto'];
export type CreateTicketInput = components['schemas']['CreateTicket'];
export type TicketChange = Omit<components['schemas']['UpdateTicket'], 'version'>;
export type StepChange = Omit<components['schemas']['ChangeStep'], 'version'>;

export const ticketKeys = {
  all: ['tickets'] as const,
  lists: () => [...ticketKeys.all, 'list'] as const,
  list: (boardId: string) => [...ticketKeys.lists(), boardId] as const,
  detail: (key: string) => [...ticketKeys.all, 'detail', key] as const,
};

const TICKET_LISTS = ticketKeys.lists();

// A ticket change also moves the open ticket count of its board.
export const ticketsLive: ChangeRegistry = {
  ticket: [ticketKeys.all, boardKeys.all],
};

export function ticketsQuery(boardId: string) {
  return queryOptions({
    queryKey: ticketKeys.list(boardId),
    queryFn: async () =>
      unwrap(await api.GET('/api/v1/tickets', { params: { query: { boardId } } })).items,
  });
}

export function ticketQuery(key: string) {
  return queryOptions({
    queryKey: ticketKeys.detail(key),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/tickets/{ticketKey}', { params: { path: { ticketKey: key } } }),
      ),
  });
}

function useTicketsChanged(lists: readonly string[] = ticketKeys.all) {
  const tickets = useInvalidate(lists);
  const boards = useInvalidate(boardKeys.all);
  return useCallback(async () => {
    await Promise.all([tickets(), boards()]);
  }, [tickets, boards]);
}

// Stores the answer so the next change starts from its version before the refetch lands.
function useDetailMutation<Vars>(mutationFn: (vars: Vars) => Promise<TicketDetail>) {
  const queryClient = useQueryClient();
  const changed = useTicketsChanged();
  return useMutation({
    mutationFn,
    onSuccess: (detail) => {
      queryClient.setQueryData(ticketKeys.detail(detail.ticket.key), detail);
    },
    onSettled: changed,
  });
}

export interface CreateTicketVars {
  projectId: string;
  boardId: string;
  input: CreateTicketInput;
}

export function useCreateTicket() {
  const changed = useTicketsChanged();
  return useMutation({
    mutationFn: async ({ projectId, boardId, input }: CreateTicketVars) =>
      unwrap(
        await api.POST('/api/v1/projects/{projectId}/boards/{boardId}/tickets', {
          params: { path: { projectId, boardId } },
          body: input,
        }),
      ),
    onSettled: changed,
  });
}

export interface UpdateTicketVars {
  key: string;
  version: number;
  change: TicketChange;
}

export function useUpdateTicket() {
  return useDetailMutation(async ({ key, version, change }: UpdateTicketVars) =>
    unwrap(
      await api.PATCH('/api/v1/tickets/{ticketKey}', {
        params: { path: { ticketKey: key } },
        body: { ...change, version },
      }),
    ),
  );
}

export interface MoveTicketVars {
  key: string;
  version: number;
  status: TicketStatus;
  acceptance?: string;
}

export function useMoveTicket() {
  const queryClient = useQueryClient();
  const changed = useTicketsChanged();
  return useMutation({
    mutationFn: async ({ key, version, status, acceptance }: MoveTicketVars) =>
      unwrap(
        await api.POST('/api/v1/tickets/{ticketKey}/move', {
          params: { path: { ticketKey: key } },
          body: { status, version, ...(acceptance === undefined ? {} : { acceptance }) },
        }),
      ),
    onSuccess: (ticket) => {
      queryClient.setQueryData<TicketDetail>(
        ticketKeys.detail(ticket.key),
        (detail) => detail && { ...detail, ticket },
      );
    },
    onSettled: changed,
  });
}

export interface DeleteTicketVars {
  key: string;
  version: number;
}

// Refetches the lists; the ticket itself only after a refusal, so a retry sends its new version.
export function useDeleteTicket() {
  const queryClient = useQueryClient();
  const changed = useTicketsChanged(TICKET_LISTS);
  return useMutation({
    mutationFn: async ({ key, version }: DeleteTicketVars) => {
      unwrapEmpty(
        await api.DELETE('/api/v1/tickets/{ticketKey}', {
          params: { path: { ticketKey: key }, query: { version } },
        }),
      );
    },
    onError: (_error, { key }) =>
      queryClient.invalidateQueries({ queryKey: ticketKeys.detail(key) }),
    onSettled: changed,
  });
}

export interface AddStepVars {
  key: string;
  text: string;
  detail?: string;
}

export function useAddStep() {
  return useDetailMutation(async ({ key, text, detail }: AddStepVars) =>
    unwrap(
      await api.POST('/api/v1/tickets/{ticketKey}/steps', {
        params: { path: { ticketKey: key } },
        body: { text, ...(detail === undefined ? {} : { detail }) },
      }),
    ),
  );
}

// `expectText` is the text the step had when it was read; the daemon refuses a changed step.
export interface ChangeStepVars {
  key: string;
  id: string;
  change: StepChange;
}

export function useChangeStep() {
  return useDetailMutation(async ({ key, id, change }: ChangeStepVars) =>
    unwrap(
      await api.PATCH('/api/v1/tickets/{ticketKey}/steps/{stepId}', {
        params: { path: { ticketKey: key, stepId: id } },
        body: change,
      }),
    ),
  );
}

export interface RemoveStepVars {
  key: string;
  id: string;
  expectText: string;
}

export function useRemoveStep() {
  return useDetailMutation(async ({ key, id, expectText }: RemoveStepVars) =>
    unwrap(
      await api.DELETE('/api/v1/tickets/{ticketKey}/steps/{stepId}', {
        params: { path: { ticketKey: key, stepId: id }, query: { expectText } },
      }),
    ),
  );
}

export interface MoveStepVars {
  key: string;
  id: string;
  to: number;
  expectText: string;
}

export function useMoveStep() {
  return useDetailMutation(async ({ key, id, to, expectText }: MoveStepVars) =>
    unwrap(
      await api.POST('/api/v1/tickets/{ticketKey}/steps/{stepId}/move', {
        params: { path: { ticketKey: key, stepId: id } },
        body: { to, expectText },
      }),
    ),
  );
}
