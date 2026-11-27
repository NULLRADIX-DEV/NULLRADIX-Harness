import { ApiProblemError } from '../../shared/api/errors';
import { problemMessage, type ProblemMessages } from '../../shared/api/problemMessage';
import type { Ticket } from '../tickets/queries';

export const ACCEPTANCE_QUESTION = 'How can one tell it is finished?';
export const CONFLICT_MESSAGE =
  'This ticket changed in the meantime. It is up to date now; check it and try again.';
export const STEP_CONFLICT_MESSAGE =
  'This step changed in the meantime. The steps are up to date now.';
export const GONE_MESSAGE = 'This ticket no longer exists.';
export const DEPENDENCY_OPEN_MESSAGE =
  'An open dependency holds this ticket back; finish it first.';

// The saved value of a field changed while the user typed over it.
export class FieldConflict {
  readonly label: string;

  constructor(label: string) {
    this.label = label;
  }
}

const TICKET_MESSAGES: ProblemMessages = {
  'version-conflict': CONFLICT_MESSAGE,
  conflict: STEP_CONFLICT_MESSAGE,
  'ticket-not-found': GONE_MESSAGE,
  'step-not-found': 'This step no longer exists. The steps are up to date now.',
  'acceptance-missing': `Done needs acceptance criteria. ${ACCEPTANCE_QUESTION}`,
  'not-needed-human-only': 'Only a person can mark a ticket as not needed.',
  'question-open': 'An open question holds this ticket back; decide it first.',
  'dependency-open': DEPENDENCY_OPEN_MESSAGE,
  'nothing-changed': 'Nothing changed.',
  'title-empty': 'The title must not be empty.',
  'step-empty': 'A step needs a text.',
  'comment-empty': 'A comment needs a text.',
  'target-branch-invalid': 'This is not a valid branch name.',
  'target-branch-locked': 'The target branch can only change before the ticket starts.',
  'blocked-reason-not-blocked': 'A reason belongs to a blocked ticket; block the ticket first.',
};

// Text for a failed change of `ticket`; open dependencies are named.
export function ticketProblem(error: unknown, ticket: Ticket): string | null {
  if (error instanceof FieldConflict) {
    return `${error.label} changed in the meantime. Your text is kept; save again to replace it.`;
  }
  const open = ticket.dependsOpen;
  const messages =
    open.length === 0
      ? TICKET_MESSAGES
      : {
          ...TICKET_MESSAGES,
          'dependency-open': `${ticket.key} waits on ${open.join(', ')}; finish those first.`,
        };
  return problemMessage(error, messages, 'Could not change the ticket.');
}

export function loadProblem(error: unknown): string | null {
  return problemMessage(error, TICKET_MESSAGES, 'Could not load the ticket.');
}

export function duplicateOf(error: unknown): string | null {
  if (error instanceof ApiProblemError && error.code === 'ticket-duplicate') return error.existing;
  return null;
}

export function createProblem(error: unknown): string | null {
  const existing = duplicateOf(error);
  if (existing !== null) return `An open ticket with this title already exists: ${existing}.`;
  return problemMessage(error, TICKET_MESSAGES, 'Could not create the ticket.');
}
