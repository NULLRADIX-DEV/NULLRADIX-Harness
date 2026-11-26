import type { TestRun } from './queries';

function badge(ok: boolean | null | undefined): { text: string; kind: string } {
  if (ok === true) return { text: 'green', kind: 'ok' };
  if (ok === false) return { text: 'red', kind: 'fail' };
  return { text: 'no result', kind: 'none' };
}

// What the agent ran to check its work, with the end of each output.
export function ReviewTests({ tests }: { tests: readonly TestRun[] }) {
  return (
    <div className="review-tests">
      <h4>Tests the agent ran</h4>
      {tests.length === 0 && (
        <p className="muted">No tests found (tests, build or lint through Bash or PowerShell).</p>
      )}
      {tests.map((test, index) => {
        const result = badge(test.ok);
        return (
          // the same command can run twice, so the order is the identity
          <details key={`${String(index)}:${test.command}`} className="review-test">
            <summary>
              <span className={`review-badge ${result.kind}`}>{result.text}</span>
              <code>{test.command}</code>
            </summary>
            <pre className="review-output">{test.output === '' ? '(no output)' : test.output}</pre>
          </details>
        );
      })}
    </div>
  );
}
