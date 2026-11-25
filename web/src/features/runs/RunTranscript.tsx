import { useState } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import { useRunTranscript } from './dispatchQueries';
import { foldResults } from './transcriptLines';

// The readable log of a run; Exact shows the whole call or message of every line.
export function RunTranscript({ runId, active }: { runId: string; active: boolean }) {
  const transcript = useRunTranscript(runId, active);
  const [exact, setExact] = useState(false);
  const items = foldResults(transcript.lines);

  return (
    <section className="run-transcript" aria-label="Transcript">
      <div className="run-section-head">
        <h4>Transcript</h4>
        <button
          type="button"
          className="btn small"
          aria-pressed={exact}
          onClick={() => {
            setExact((current) => !current);
          }}
        >
          Exact
        </button>
      </div>
      {items.length > 0 && (
        <ol className="run-steps">
          {items.map(({ line, outcome, failure }, index) => (
            <li
              key={`${String(line.seq)}-${String(index)}`}
              className={`run-step run-step-${line.kind}${outcome ? ` run-step-${outcome}` : ''}`}
            >
              <span className="run-step-kind muted">{line.kind}</span>
              {exact ? (
                <pre className="run-step-text">{line.detail ?? line.text}</pre>
              ) : (
                <span className="run-step-text">{line.text}</span>
              )}
              {failure && <span className="run-step-failure">{failure}</span>}
            </li>
          ))}
        </ol>
      )}
      {items.length === 0 && !transcript.refused && (
        <p className="muted">{transcript.finished ? 'Nothing was logged.' : 'No lines yet.'}</p>
      )}
      <FormError
        message={problemMessage(transcript.refused, {}, 'Could not load the transcript.')}
      />
      {transcript.retrying && <p className="form-error">Cannot reach the transcript, retrying.</p>}
    </section>
  );
}
