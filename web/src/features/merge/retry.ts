import { useMutation } from '@tanstack/react-query';
import { useCallback } from 'react';
import { api } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import { useInvalidate } from '../../shared/query/useInvalidate';
import { boardKeys } from '../boards/queries';
import { inboxKeys } from '../inbox/queries';
import { ticketKeys } from '../tickets/queries';
import { mergeKeys } from './queries';

export interface RetryVars {
  ticketKey: string;
  // The inbox does not know the version of the ticket.
  version?: number;
}

// A retry moves the ticket, its board counts, the inbox and the merge list.
export function useRetryMerge() {
  const tickets = useInvalidate(ticketKeys.all);
  const boards = useInvalidate(boardKeys.all);
  const inbox = useInvalidate(inboxKeys.all);
  const merges = useInvalidate(mergeKeys.all);
  const settled = useCallback(async () => {
    await Promise.all([tickets(), boards(), inbox(), merges()]);
  }, [tickets, boards, inbox, merges]);
  return useMutation({
    mutationFn: async ({ ticketKey, version }: RetryVars) =>
      unwrap(
        await api.POST('/api/v1/tickets/{ticketKey}/merge', {
          params: { path: { ticketKey } },
          body: version === undefined ? {} : { version },
        }),
      ),
    onSettled: settled,
  });
}
