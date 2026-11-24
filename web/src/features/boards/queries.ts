import type { ChangeRegistry } from '../../shared/live/registry';

export const boardKeys = {
  all: ['boards'] as const,
};

export const boardsLive: ChangeRegistry = {
  board: [boardKeys.all],
};
