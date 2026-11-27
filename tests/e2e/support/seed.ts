import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import type { Daemon } from './daemon';
import { text } from './json';

export interface Seed {
  projectId: string;
  boardId: string;
  repository: string;
}

const FILES: Record<string, string> = {
  'README.md': '# Arches\n',
  'src/price.ts': 'export const price = (net: number) => net;\n',
  'src/old.ts': 'export const OLD = true;\n',
  'docs/note.md': 'The prices\nare in\nsrc/price.ts\nand nowhere else\nat all.\n',
};

// A project ARC with a board; with a repository, the board works on a git repository on main.
export async function seedProject(
  daemon: Daemon,
  options: { repository: boolean } = { repository: false },
): Promise<Seed> {
  const project = await daemon.api('POST', '/api/v1/projects', { key: 'ARC', name: 'Arches' });
  const projectId = text(project, 'id');
  const board = await daemon.api('POST', `/api/v1/projects/${projectId}/boards`, { name: 'Main' });
  const boardId = text(board, 'id');
  const repository = join(daemon.work, 'arches');
  if (options.repository) {
    await mkdir(repository, { recursive: true });
    for (const [name, content] of Object.entries(FILES)) {
      const file = join(repository, name);
      await mkdir(dirname(file), { recursive: true });
      await writeFile(file, content);
    }
    await daemon.git(repository, ['init', '-q', '-b', 'main']);
    await daemon.git(repository, ['add', '-A']);
    await daemon.git(repository, ['commit', '-q', '-m', 'init']);
    await daemon.cli(['board', 'repo', boardId, repository]);
  }
  return { projectId, boardId, repository };
}
