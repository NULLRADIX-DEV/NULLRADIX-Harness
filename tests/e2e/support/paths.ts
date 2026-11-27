import { join, resolve } from 'node:path';

const exe = process.platform === 'win32' ? '.exe' : '';

export const repositoryRoot = resolve(import.meta.dirname, '..', '..', '..');

const targetDir = process.env['CARGO_TARGET_DIR'] ?? join(repositoryRoot, 'target');

export const nullradixBinary = join(targetDir, 'debug', `nullradix${exe}`);
export const fakeClaudeBinary = join(targetDir, 'debug', `nullradix-fake-claude${exe}`);
export const webDist = join(repositoryRoot, 'web', 'dist');
