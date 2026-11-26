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
