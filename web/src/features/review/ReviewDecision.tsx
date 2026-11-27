import { useId, useState } from 'react';
import { useAcceptReview, useReturnReview } from './decisions';
import { decisionText, FEEDBACK_MAX } from './messages';
import { Button } from '../../shared/ui/Button';

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
            <Button type="submit" variant="primary" disabled={!ready || busy}>
              Return with feedback
            </Button>
            <Button
              type="button"
              disabled={busy}
              onClick={() => {
                setReturning(false);
                send.reset();
              }}
            >
              Cancel
            </Button>
          </div>
        </form>
      ) : (
        <div className="actions">
          <Button
            type="button"
            variant="primary"
            disabled={busy}
            onClick={() => {
              accept.mutate({ ticketKey, version });
            }}
          >
            Accept
          </Button>
          <Button
            type="button"
            disabled={busy}
            onClick={() => {
              setReturning(true);
              accept.reset();
            }}
          >
            Return to agent
          </Button>
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
