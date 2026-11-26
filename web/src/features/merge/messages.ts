import { problemMessage, type ProblemMessages } from '../../shared/api/problemMessage';

const FALLBACK = 'Could not start the merge.';

const MESSAGES: ProblemMessages = {
  'merge-not-possible': 'This ticket waits for no merge any more.',
  'version-conflict': 'The ticket changed meanwhile. Reload it and try again.',
  'ticket-not-found': 'This ticket no longer exists.',
};

export function retryText(error: unknown): string {
  return problemMessage(error, MESSAGES, FALLBACK) ?? FALLBACK;
}

// A done ticket with a branch, or one blocked by a merge problem, can be merged again.
export function canRetry(
  status: string,
  reason: string | null | undefined,
  branch: string | null | undefined,
): boolean {
  if (!branch) return false;
  return status === 'done' || (status === 'blocked' && (reason ?? '').trim().startsWith('Merge:'));
}
