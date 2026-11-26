import { useId, useState } from 'react';
import { useAcceptReview, useReturnReview } from './decisions';
import { decisionText, FEEDBACK_MAX } from './messages';

// A person accepts the held ticket or sends it back to the agent with feedback.
export function ReviewDecision({ ticketKey, version }: { ticketKey: string; version: number }) {
  const accept = useAcceptReview();
  const send = useReturnReview();
  const [returning, setReturning] = useState(false);
  const [feedback, setFeedback] = useState('');
  const fieldId = useId();
  const busy = accept.isPending || send.isPending;
  const error = send.error ?? accept.error;
  const ready = feedback.trim() !== '';
  return (
    <div className="review-decision">
      {returning ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (ready && !busy) send.mutate({ ticketKey, version, feedback: feedback.trim() });
          }}
        >
          <label htmlFor={fieldId}>Feedback for the agent</label>
          <textarea
            id={fieldId}
            rows={4}
            maxLength={FEEDBACK_MAX}
            value={feedback}
            onChange={(event) => {
              setFeedback(event.target.value);
            }}
          />
          <div className="actions">
            <button type="submit" className="btn primary" disabled={!ready || busy}>
              Return with feedback
            </button>
            <button
              type="button"
              className="btn"
              disabled={busy}
              onClick={() => {
                setReturning(false);
                send.reset();
              }}
            >
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <div className="actions">
          <button
            type="button"
            className="btn primary"
            disabled={busy}
            onClick={() => {
              accept.mutate({ ticketKey, version });
            }}
          >
            Accept
          </button>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => {
              setReturning(true);
              accept.reset();
            }}
          >
            Return to agent
          </button>
        </div>
      )}
      {error && (
        <p role="alert" className="form-error">
          {decisionText(error)}
        </p>
      )}
    </div>
  );
}
