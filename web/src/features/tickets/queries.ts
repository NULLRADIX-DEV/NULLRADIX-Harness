import type { ChangeRegistry } from '../../shared/live/registry';
import { boardKeys } from '../boards/queries';

export const ticketKeys = {
  all: ['tickets'] as const,
};

// A ticket change also moves the open ticket count of its board.
export const ticketsLive: ChangeRegistry = {
  ticket: [ticketKeys.all, boardKeys.all],
};
