import { useQuery } from '@tanstack/react-query';
import { problemText } from './messages';
import { reviewQuery } from './queries';
import { ReviewDecision } from './ReviewDecision';
import { ReviewFiles } from './ReviewFiles';
import { ReviewTests } from './ReviewTests';
import { ARROW } from './symbols';
import './review.css';

const BASE_LENGTH = 8;

// The branch of a ticket against its target, and the tests of the run that finished it.
export function TicketReview({ ticketKey, version }: { ticketKey: string; version: number }) {
  const { status, data, error } = useQuery(reviewQuery(ticketKey));
  if (status === 'pending') return <p className="muted">Loading the review...</p>;
  if (status === 'error') return <p className="form-error">{problemText(error)}</p>;
  return (
    <div className="review">
      <p className="review-head">
        <span className="mono">{data.branch ?? 'No branch'}</span>
        {data.target && (
          <>
            {` ${ARROW} `}
            <span className="mono">{data.target}</span>
            {data.base && <span className="muted"> from {data.base.slice(0, BASE_LENGTH)}</span>}
          </>
        )}
      </p>
      {data.state === 'pending' && (
        <>
          <p className="muted">The ticket waits for a review before it is merged.</p>
          <ReviewDecision ticketKey={ticketKey} version={version} />
        </>
      )}
      {!data.available && data.reason && <p className="muted">{data.reason}</p>}
      {data.available && (
        <ReviewFiles ticketKey={ticketKey} files={data.files} truncated={data.filesTruncated} />
      )}
      {data.state !== null && data.state !== undefined && <ReviewTests tests={data.tests} />}
    </div>
  );
}
