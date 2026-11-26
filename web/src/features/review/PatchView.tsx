import { useQuery } from '@tanstack/react-query';
import { problemText } from './messages';
import { patchQuery } from './queries';

function lineClass(line: string): string | undefined {
  if (line.startsWith('@@')) return 'hunk';
  if (line.startsWith('+++') || line.startsWith('---')) return 'meta';
  if (line.startsWith('+')) return 'add';
  if (line.startsWith('-')) return 'del';
  return undefined;
}

// The patch of one file, loaded once it is shown.
export function PatchView({ ticketKey, path }: { ticketKey: string; path: string }) {
  const { status, data, error } = useQuery(patchQuery(ticketKey, path));
  if (status === 'pending') return <p className="muted">Loading the changes...</p>;
  if (status === 'error') return <p className="form-error">{problemText(error)}</p>;
  const lines = data.patch.replace(/\n$/, '').split('\n');
  return (
    <>
      <pre className="review-patch" aria-label={`Changes of ${path}`}>
        {lines.map((line, index) => (
          // lines repeat, so their place in the patch is their identity
          <span key={`${String(index)}:${line}`} className={lineClass(line)}>{`${line}\n`}</span>
        ))}
      </pre>
      {data.truncated && <p className="muted">The patch is very large and was cut.</p>}
    </>
  );
}
