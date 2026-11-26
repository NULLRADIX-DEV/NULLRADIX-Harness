import { describe, expect, it } from 'vitest';
import type { ReviewFile } from './queries';
import { reviewTree, type ReviewDir } from './reviewTree';

function file(path: string, additions = 1, deletions = 0): ReviewFile {
  return { path, status: 'M', additions, deletions, binary: false };
}

describe('reviewTree', () => {
  it('lists folders before files, each alphabetical, and counts below a folder', () => {
    const tree = reviewTree([
      file('README.md'),
      file('src/b.ts', 2, 1),
      file('docs/a.md'),
      file('src/a.ts', 3, 0),
      file('docs/z/deep.md'),
    ]);
    expect(tree.map((node) => node.name)).toEqual(['docs', 'src', 'README.md']);
    const [docs, src] = tree as ReviewDir[];
    expect(src?.files).toBe(2);
    expect([src?.additions, src?.deletions]).toEqual([5, 1]);
    expect(docs?.files).toBe(2);
    expect(src?.children.map((node) => node.name)).toEqual(['a.ts', 'b.ts']);
  });

  it('joins a chain of folders that hold nothing else', () => {
    const [chain] = reviewTree([file('crates/app/src/review.rs'), file('crates/app/src/lib.rs')]);
    expect(chain?.kind).toBe('dir');
    expect(chain?.name).toBe('crates/app/src');
    expect((chain as ReviewDir).path).toBe('crates/app/src');
  });

  it('keeps a folder with a file and a folder apart', () => {
    const [crates] = reviewTree([file('crates/Cargo.toml'), file('crates/app/lib.rs')]) as [
      ReviewDir,
    ];
    expect(crates.name).toBe('crates');
    expect(crates.children.map((node) => node.name)).toEqual(['app', 'Cargo.toml']);
  });

  it('is empty without files', () => {
    expect(reviewTree([])).toEqual([]);
  });
});
