import { useQuery } from '@tanstack/react-query';
import { canRetry, retryText } from './messages';
import { mergesQuery, type MergeJob } from './queries';
import { useRetryMerge } from './retry';

const PHASES: Readonly<Record<MergeJob['phase'], string>> = {
  pending: 'Queued',
  waiting: 'Waiting',
  'conflict-session': 'Merge session',
  merged: 'Merged',
  failed: 'Failed',
  cancelled: 'Cancelled',
};

function JobLine({ job }: { job: MergeJob }) {
  const target = job.targetBranch ?? 'the checked-out branch';
  const result =
    job.phase === 'merged' && job.mergedInto && job.mergedCommit
      ? ` into ${job.mergedInto} (${job.mergedCommit.slice(0, 7)})`
      : ` into ${target}`;
  const phase =
    job.phase === 'conflict-session' && job.conflictSessions > 1
      ? `${PHASES[job.phase]} ${String(job.conflictSessions)}`
      : PHASES[job.phase];
  return (
    <li className="merge-job">
      <span className="badge">{phase}</span> <span className="mono">{job.branch}</span>
      {result}
      <span className="muted">{` / ${new Date(job.queuedAt).toLocaleString()}`}</span>
      {job.problem && <p className="muted">{job.problem}</p>}
      {job.phase === 'waiting' && job.conflictSeen && (
        <p className="muted">A merge session starts when the dispatcher may start one.</p>
      )}
    </li>
  );
}

// The merges of the branch of a ticket, newest first, and the way to try again.
export function TicketMerges({
  ticketKey,
  version,
  status,
  blockedReason,
  branch,
}: {
  ticketKey: string;
  version: number;
  status: string;
  blockedReason: string | null | undefined;
  branch: string | null | undefined;
}) {
  const { status: loaded, data } = useQuery(mergesQuery(ticketKey));
  const retry = useRetryMerge();
  if (loaded === 'pending') return <p className="muted">Loading the merges...</p>;
  if (loaded === 'error') return <p className="form-error">Could not load the merges.</p>;
  return (
    <div className="merges">
      {canRetry(status, blockedReason, branch) && (
        <div className="actions">
          <button
            type="button"
            className="btn primary"
            disabled={retry.isPending}
            onClick={() => {
              retry.mutate({ ticketKey, version });
            }}
          >
            Retry merge
          </button>
        </div>
      )}
      {retry.data && <p role="status">{retry.data.text}</p>}
      {retry.error && (
        <p role="alert" className="form-error">
          {retryText(retry.error)}
        </p>
      )}
      {data.length === 0 ? (
        <p className="muted">This ticket has not been merged yet.</p>
      ) : (
        <ul className="merge-jobs">
          {data.map((job) => (
            <JobLine key={job.id} job={job} />
          ))}
        </ul>
      )}
    </div>
  );
}
