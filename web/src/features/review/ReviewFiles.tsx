import type { ReviewFile } from './queries';
import { ReviewFileItem } from './ReviewFileItem';
import { reviewTree, type ReviewNode } from './reviewTree';
import { DOT, lines } from './symbols';

// Folders start open up to this many files.
const OPEN_DIRS_UP_TO = 5;

function plural(count: number, one: string, many: string): string {
  return `${String(count)} ${count === 1 ? one : many}`;
}

function Nodes({
  ticketKey,
  nodes,
  open,
}: {
  ticketKey: string;
  nodes: readonly ReviewNode[];
  open: boolean;
}) {
  return (
    <div className="review-level">
      {nodes.map((node) =>
        node.kind === 'file' ? (
          <ReviewFileItem
            key={node.file.path}
            ticketKey={ticketKey}
            file={node.file}
            name={node.name}
          />
        ) : (
          <details key={node.path} className="review-dir" open={open}>
            <summary title={node.path}>
              <span className="review-folder" aria-hidden="true" />
              <span className="review-path">{node.name}</span>
              <span className="review-count">
                {`${plural(node.files, 'file', 'files')} ${DOT} ${lines(node.additions, node.deletions)}`}
              </span>
            </summary>
            <Nodes ticketKey={ticketKey} nodes={node.children} open={open} />
          </details>
        ),
      )}
    </div>
  );
}

// The changed files as a tree; a patch loads when its file is opened.
export function ReviewFiles({
  ticketKey,
  files,
  truncated,
}: {
  ticketKey: string;
  files: readonly ReviewFile[];
  truncated: boolean;
}) {
  if (files.length === 0) return <p className="muted">No changes against the target.</p>;
  const additions = files.reduce((sum, file) => sum + file.additions, 0);
  const deletions = files.reduce((sum, file) => sum + file.deletions, 0);
  return (
    <div className="review-files">
      <p className="muted">
        {`${plural(files.length, 'file', 'files')} changed, ${lines(additions, deletions)}`}
      </p>
      {truncated && <p className="muted">There are more changed files than are listed here.</p>}
      <Nodes
        ticketKey={ticketKey}
        nodes={reviewTree(files)}
        open={files.length <= OPEN_DIRS_UP_TO}
      />
    </div>
  );
}
