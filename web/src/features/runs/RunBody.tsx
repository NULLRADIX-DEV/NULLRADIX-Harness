import { useRunDetail } from './dispatchQueries';
import { runChips, shortModel, usageLine } from './format';
import { isActive, type Run } from './queries';
import { RunShots } from './RunShots';
import { RunTranscript } from './RunTranscript';

function Usage({ run }: { run: Run }) {
  const usage = run.result?.usage;
  if (!usage) return null;
  const models = usage.models.length > 1 ? usage.models : [];
  return (
    <section className="run-usage" aria-label="Usage">
      <h4>Usage</h4>
      <p>{usageLine(usage)}</p>
      {models.length > 0 && (
        <ul>
          {models.map((model) => (
            <li key={model.model}>
              <span className="mono">{shortModel(model.model)}</span>: {usageLine(model)}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

// What a run did: settings, prompt, transcript, refused tools, screenshots and usage.
export function RunBody({ run }: { run: Run }) {
  const { data: detail, isError } = useRunDetail(run.id);
  const chips = runChips(run.dispatch);
  const abort = run.result?.abort;
  return (
    <div className="run-body">
      {chips.length > 0 && (
        <ul className="run-chips" aria-label="Settings of the run">
          {chips.map((chip) => (
            <li key={chip}>{chip}</li>
          ))}
        </ul>
      )}
      {abort && <p className="run-abort-reason">{abort.reason}</p>}
      {run.error && !abort && <p className="form-error">{run.error}</p>}
      {isError && <p className="form-error">Could not load the run details.</p>}
      {detail?.prompt && (
        <details className="run-prompt">
          <summary>Prompt</summary>
          <pre>{detail.prompt}</pre>
        </details>
      )}
      <RunTranscript runId={run.id} active={isActive(run.status)} />
      {detail && detail.deniedTools.length > 0 && (
        <section className="run-denied" aria-label="Denied tools">
          <h4>Denied tools</h4>
          <ul>
            {detail.deniedTools.map((denied) => (
              <li key={denied.tool}>
                <span className="mono">{denied.tool}</span> {`x ${String(denied.count)}`}
              </li>
            ))}
          </ul>
        </section>
      )}
      {detail && <RunShots runId={run.id} shots={detail.screenshots} />}
      <Usage run={run} />
    </div>
  );
}
