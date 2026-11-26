import { useMutation } from '@tanstack/react-query';
import { useCallback } from 'react';
import { api } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import { useInvalidate } from '../../shared/query/useInvalidate';
import { boardKeys } from '../boards/queries';
import { ticketKeys } from '../tickets/queries';
import { reviewKeys } from './queries';

// A decision moves the ticket, its board counts and the review itself.
function useDecided() {
  const tickets = useInvalidate(ticketKeys.all);
  const boards = useInvalidate(boardKeys.all);
  const reviews = useInvalidate(reviewKeys.all);
  return useCallback(async () => {
    await Promise.all([tickets(), boards(), reviews()]);
  }, [tickets, boards, reviews]);
}

export interface AcceptVars {
  ticketKey: string;
  version: number;
}

export function useAcceptReview() {
  const decided = useDecided();
  return useMutation({
    mutationFn: async ({ ticketKey, version }: AcceptVars) =>
      unwrap(
        await api.POST('/api/v1/tickets/{ticketKey}/review/accept', {
          params: { path: { ticketKey } },
          body: { version },
        }),
      ),
    onSettled: decided,
  });
}

export interface ReturnVars extends AcceptVars {
  feedback: string;
}

export function useReturnReview() {
  const decided = useDecided();
  return useMutation({
    mutationFn: async ({ ticketKey, version, feedback }: ReturnVars) =>
      unwrap(
        await api.POST('/api/v1/tickets/{ticketKey}/review/return', {
          params: { path: { ticketKey } },
          body: { version, feedback },
        }),
      ),
    onSettled: decided,
  });
}
