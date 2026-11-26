import { problemMessage, type ProblemMessages } from '../../shared/api/problemMessage';

const FALLBACK = 'Could not load the review.';

const MESSAGES: ProblemMessages = {
  'review-not-available': 'There is nothing to review for this ticket.',
  'review-file-unknown': 'This file is not on the branch.',
  'review-branch-invalid': 'The branch of this ticket has a name the review cannot use.',
  'git-failed': 'Git could not read the branch.',
  'ticket-not-found': 'This ticket no longer exists.',
};

export function problemText(error: unknown): string {
  return problemMessage(error, MESSAGES, FALLBACK) ?? FALLBACK;
}

// What the agent is told at most; the daemon refuses more.
export const FEEDBACK_MAX = 10_000;

const DECISION_FALLBACK = 'Could not save the decision.';

const DECISION_MESSAGES: ProblemMessages = {
  'accept-not-possible': 'This ticket waits for no review any more.',
  'return-not-possible': 'This ticket waits for no review any more.',
  'feedback-empty': 'Write what the agent should change.',
  'value-too-long': 'The feedback is too long.',
  'version-conflict': 'The ticket changed meanwhile. Reload it and try again.',
  'ticket-not-found': 'This ticket no longer exists.',
};

export function decisionText(error: unknown): string {
  return problemMessage(error, DECISION_MESSAGES, DECISION_FALLBACK) ?? DECISION_FALLBACK;
}
