import { useState } from 'react';
import { FileView } from './FileView';
import { PatchView } from './PatchView';
import type { ReviewFile } from './queries';
import { ARROW, lines } from './symbols';
import { Button } from '../../shared/ui/Button';

const STATUS_LABELS: Readonly<Record<string, string>> = {
  A: 'new',
  M: 'changed',
  D: 'deleted',
  R: 'renamed',
  C: 'copied',
  T: 'type changed',
};

type Shown = 'changes' | 'file';

function lastSegment(path: string): string {
  return path.split('/').pop() ?? path;
}

// One changed file: status and lines in the head, the patch or the file once opened.
export function ReviewFileItem({
  ticketKey,
  file,
  name,
}: {
  ticketKey: string;
  file: ReviewFile;
  name: string;
}) {
  const [open, setOpen] = useState(false);
  const [shown, setShown] = useState<Shown>('changes');
  const deleted = file.status === 'D';
  const label = file.oldPath ? `${lastSegment(file.oldPath)} ${ARROW} ${name}` : name;
  const title = file.oldPath ? `${file.oldPath} ${ARROW} ${file.path}` : file.path;
  return (
    <details
      className="review-file-item"
      onToggle={(event) => {
        setOpen(event.currentTarget.open);
      }}
    >
      <summary>
        <span className={`review-status s-${file.status}`}>
          {STATUS_LABELS[file.status] ?? file.status}
        </span>
        <span className="review-path" title={title}>
          {label}
        </span>
        <span className="review-count">
          {file.binary ? 'binary' : lines(file.additions, file.deletions)}
        </span>
      </summary>
      {open && (
        <>
          <div className="actions review-switch" role="group" aria-label={`Show ${file.path}`}>
            <Button
              type="button"
              aria-pressed={shown === 'changes' || deleted}
              onClick={() => {
                setShown('changes');
              }}
            >
              Changes
            </Button>
            <Button
              type="button"
              aria-pressed={shown === 'file' && !deleted}
              disabled={deleted}
              {...(deleted && { title: 'The file is not on the branch any more' })}
              onClick={() => {
                setShown('file');
              }}
            >
              File
            </Button>
          </div>
          {shown === 'changes' || deleted ? (
            <PatchView ticketKey={ticketKey} path={file.path} />
          ) : (
            <FileView ticketKey={ticketKey} path={file.path} />
          )}
        </>
      )}
    </details>
  );
}
