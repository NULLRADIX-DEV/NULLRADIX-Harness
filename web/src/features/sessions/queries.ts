import { queryOptions } from '@tanstack/react-query';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';

export type Session = components['schemas']['SessionDto'];
export type DisplayState = components['schemas']['DisplayStateDto'];
export type Subagent = components['schemas']['SubagentDto'];

export const sessionKeys = {
  all: ['sessions'] as const,
  list: () => [...sessionKeys.all, 'list'] as const,
  ticket: (key: string) => [...sessionKeys.all, 'ticket', key] as const,
};

export const sessionsLive: ChangeRegistry = {
  session: [sessionKeys.all],
};

export function sessionsQuery() {
  return queryOptions({
    queryKey: sessionKeys.list(),
    queryFn: async () => unwrap(await api.GET('/api/v1/sessions')).items,
  });
}

export function ticketSessionsQuery(ticketKey: string) {
  return queryOptions({
    queryKey: sessionKeys.ticket(ticketKey),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/tickets/{ticketKey}/sessions', {
          params: { path: { ticketKey } },
        }),
      ).items,
  });
}
