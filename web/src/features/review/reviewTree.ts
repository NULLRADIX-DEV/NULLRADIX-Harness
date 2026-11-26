import type { ReviewFile } from './queries';

export interface ReviewDir {
  kind: 'dir';
  // A chain of folders with one folder each shows as one name, such as "src/dispatch".
  name: string;
  path: string;
  children: ReviewNode[];
  // Files below, on every level, and their lines.
  files: number;
  additions: number;
  deletions: number;
}

export interface ReviewLeaf {
  kind: 'file';
  name: string;
  file: ReviewFile;
}

export type ReviewNode = ReviewDir | ReviewLeaf;

function emptyDir(name: string, path: string): ReviewDir {
  return { kind: 'dir', name, path, children: [], files: 0, additions: 0, deletions: 0 };
}

// Folders first, alphabetical; a folder with nothing but one folder joins it.
export function reviewTree(files: readonly ReviewFile[]): ReviewNode[] {
  const root = emptyDir('', '');
  for (const file of files) {
    const parts = file.path.split('/');
    let dir = root;
    for (const part of parts.slice(0, -1)) {
      const path = dir.path === '' ? part : `${dir.path}/${part}`;
      let next = dir.children.find(
        (child): child is ReviewDir => child.kind === 'dir' && child.path === path,
      );
      if (!next) {
        next = emptyDir(part, path);
        dir.children.push(next);
      }
      dir = next;
    }
    dir.children.push({ kind: 'file', name: parts[parts.length - 1] ?? file.path, file });
  }
  return finish(root).children;
}

function finish(dir: ReviewDir): ReviewDir {
  dir.children = dir.children.map((child) =>
    child.kind === 'dir' ? collapse(finish(child)) : child,
  );
  dir.children.sort((a, b) =>
    a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === 'dir' ? -1 : 1,
  );
  for (const child of dir.children) {
    dir.files += child.kind === 'dir' ? child.files : 1;
    dir.additions += child.kind === 'dir' ? child.additions : child.file.additions;
    dir.deletions += child.kind === 'dir' ? child.deletions : child.file.deletions;
  }
  return dir;
}

function collapse(dir: ReviewDir): ReviewDir {
  const only = dir.children.length === 1 ? dir.children[0] : undefined;
  if (only?.kind !== 'dir') return dir;
  return { ...only, name: `${dir.name}/${only.name}` };
}
