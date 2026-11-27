import { existsSync } from 'node:fs';
import { fakeClaudeBinary, nullradixBinary, webDist } from './paths';

const BUILD = 'npm run build -w tests/e2e';

const PARTS = [
  { path: nullradixBinary, what: 'the daemon' },
  { path: fakeClaudeBinary, what: 'the fake Claude' },
  { path: webDist, what: 'the web UI' },
];

// Builds nothing: fails early with the command that makes the missing parts.
export default function globalSetup(): void {
  const missing = PARTS.filter((part) => !existsSync(part.path));
  if (missing.length > 0) {
    const names = missing.map((part) => `${part.what} (${part.path})`).join(', ');
    throw new Error(`Missing ${names}. Build them first with \`${BUILD}\`.`);
  }
}
