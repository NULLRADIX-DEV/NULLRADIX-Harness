import { useQuery } from '@tanstack/react-query';
import { problemText } from './messages';
import { fileQuery } from './queries';

function size(bytes: number): string {
  if (bytes < 1024) return `${String(bytes)} B`;
  return `${(bytes / 1024).toFixed(1)} KB`;
}

// A file as it is at the head of the branch.
export function FileView({ ticketKey, path }: { ticketKey: string; path: string }) {
  const { status, data, error } = useQuery(fileQuery(ticketKey, path));
  if (status === 'pending') return <p className="muted">Loading the file...</p>;
  if (status === 'error') return <p className="form-error">{problemText(error)}</p>;
  if (data.binary || data.text === null || data.text === undefined) {
    return <p className="muted">A binary file of {size(data.size)}.</p>;
  }
  return (
    <>
      <pre className="review-patch review-file" aria-label={`Content of ${path}`}>
        {data.text}
      </pre>
      {data.truncated && (
        <p className="muted">The file has {size(data.size)}; only its start is shown.</p>
      )}
    </>
  );
}
