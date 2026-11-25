import { useQuery } from '@tanstack/react-query';
import { useId, useState } from 'react';
import {
  fileDiffQuery,
  sessionChangesQuery,
  type ChangedFile,
  type DiffHunk,
  type SessionChanges,
} from './queries';

const STATUS: Readonly<Record<ChangedFile['status'], { letter: string; label: string }>> = {
  added: { letter: 'A', label: 'added' },
  modified: { letter: 'M', label: 'modified' },
  deleted: { letter: 'D', label: 'deleted' },
  renamed: { letter: 'R', label: 'renamed' },
};

const SIGN = { add: '+', remove: '-', context: ' ' } as const;

// Start lines from "@@ -12,7 +12,9 @@"; without such a header no line numbers.
export function hunkStart(header: string): { old: number; new: number } | null {
  const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(header);
  if (!match) return null;
  return { old: Number(match[1]), new: Number(match[2]) };
}

function Hunk({ hunk }: { hunk: DiffHunk }) {
  const start = hunkStart(hunk.header);
  let oldLine = start?.old ?? 0;
  let newLine = start?.new ?? 0;
  return (
    <div className="fd-hunk">
      <div className="fd-head mono">{hunk.header}</div>
      {hunk.lines.map((line, index) => {
        const left = line.kind !== 'add' && start ? oldLine++ : null;
        const right = line.kind !== 'remove' && start ? newLine++ : null;
        return (
          <div key={index} className={`fd-line ${line.kind}`}>
            <span className="fd-num" aria-hidden="true">
              {left ?? ''}
            </span>
            <span className="fd-num" aria-hidden="true">
              {right ?? ''}
            </span>
            <span className="fd-sign" aria-hidden="true">
              {SIGN[line.kind]}
            </span>
            <span className="mono">{line.text}</span>
          </div>
        );
      })}
    </div>
  );
}

function FileDiffView({ sessionId, path }: { sessionId: string; path: string }) {
  const { status, data } = useQuery(fileDiffQuery(sessionId, path));
  if (status === 'pending') return <p className="muted">Loading the diff...</p>;
  if (status === 'error') return <p className="form-error">Could not load the diff.</p>;
  return (
    <div className="fd" aria-label={`Diff of ${path}`}>
      {data.hunks.length === 0 ? (
        <p className="muted">No text difference, such as a binary file.</p>
      ) : (
        data.hunks.map((hunk, index) => <Hunk key={index} hunk={hunk} />)
      )}
      {data.truncated && <p className="muted">The diff is cut after 5000 lines.</p>}
    </div>
  );
}

function FileRow({ sessionId, file }: { sessionId: string; file: ChangedFile }) {
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  const status = STATUS[file.status];
  const slash = file.path.lastIndexOf('/');
  return (
    <li className="fc-file">
      <button
        type="button"
        className="fc-head"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => {
          setOpen((value) => !value);
        }}
      >
        <span className={`fc-status ${file.status}`} title={status.label}>
          {status.letter}
        </span>
        <span className="fc-path mono">
          <span className="muted">{file.path.slice(0, slash + 1)}</span>
          {file.path.slice(slash + 1)}
        </span>
        <span className="fc-counts mono">
          <span className="fc-add">+{String(file.added)}</span>{' '}
          <span className="fc-remove">-{String(file.removed)}</span>
        </span>
      </button>
      {open && (
        <div id={bodyId}>
          <FileDiffView sessionId={sessionId} path={file.path} />
        </div>
      )}
    </li>
  );
}

function Origin({ changes }: { changes: SessionChanges }) {
  if (changes.origin === 'git') {
    return (
      <p className="muted">
        From git against <span className="mono">{changes.base ?? 'HEAD'}</span>, committed and open
        changes in the working tree.
      </p>
    );
  }
  if (changes.origin === 'transcript') {
    return <p className="muted">Put together from the transcript (Edit, MultiEdit, Write).</p>;
  }
  return null;
}

// Changed files from git or the transcript; a file opens its diff.
export function ChangesView({ sessionId }: { sessionId: string }) {
  const { status, data } = useQuery(sessionChangesQuery(sessionId));
  if (status === 'pending') return <p className="muted">Loading the changes...</p>;
  if (status === 'error') return <p className="form-error">Could not load the changes.</p>;
  const added = data.files.reduce((sum, file) => sum + file.added, 0);
  const removed = data.files.reduce((sum, file) => sum + file.removed, 0);
  return (
    <div className="fc">
      <Origin changes={data} />
      {data.note && (
        <p className="fc-note" role="note">
          {data.note}
        </p>
      )}
      {data.files.length === 0 ? (
        <div className="state empty">
          <h2>No changes</h2>
          <p>
            {data.origin === 'none'
              ? 'Neither git nor the transcript show changed files.'
              : 'This session has not changed any files yet.'}
          </p>
        </div>
      ) : (
        <>
          <p className="fc-summary">
            <strong>
              {data.files.length === 1 ? '1 file' : `${String(data.files.length)} files`}
            </strong>{' '}
            <span className="fc-add mono">+{String(added)}</span>{' '}
            <span className="fc-remove mono">-{String(removed)}</span>
          </p>
          <ul className="fc-list" aria-label="Changed files">
            {data.files.map((file) => (
              <FileRow key={file.path} sessionId={sessionId} file={file} />
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
