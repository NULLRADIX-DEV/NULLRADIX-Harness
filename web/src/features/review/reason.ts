const REVIEW_REASON = 'Human Interaction: review before merge';

// Whether the ticket is blocked for the review a person decides.
export function waitsForReview(status: string, reason: string | null | undefined): boolean {
  return status === 'blocked' && reason?.trim() === REVIEW_REASON;
}
