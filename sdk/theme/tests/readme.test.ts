import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const root = join(import.meta.dirname, '..');
const readmePath = join(root, 'README.md');
const readme = () => (existsSync(readmePath) ? readFileSync(readmePath, 'utf8') : '');
const src = (name: string) => readFileSync(join(root, 'src', name), 'utf8');

// Names the README does not show as code (`Name`, also as a call `name()` or generic `Name<S>`)
const missing = (names: string[]) => {
  const text = readme();
  return names.filter((n) => !['`', '(', '<'].some((end) => text.includes('`' + n + end)));
};

// Slot names from `ThemeSlots` plus the page slots `page:<name>` from `PageName`
function slotNames(): string[] {
  const slots = src('slots.ts');
  const block = /export interface ThemeSlots[^{]*\{([\s\S]*?)\n\}/.exec(slots)?.[1] ?? '';
  const names = [...block.matchAll(/^\s*(\w+)\?:/gm)].map((m) => m[1] ?? '');
  const pages = /export type PageName\s*=\s*([^;]+);/.exec(slots)?.[1] ?? '';
  return [...names, ...[...pages.matchAll(/'([^']+)'/g)].map((m) => `page:${m[1] ?? ''}`)];
}

// Every name `src/index.ts` exports (values and the types of the re-exported files)
function exportNames(): string[] {
  const index = src('index.ts');
  const values = [...index.matchAll(/export \{([^}]+)\}/g)].flatMap((m) =>
    (m[1] ?? '').split(',').map((n) => n.trim()),
  );
  const typeFiles = [...index.matchAll(/export type \* from '\.\/(\w+)\.js'/g)].map((m) => m[1]);
  const files = readdirSync(join(root, 'src'));
  const types = typeFiles.flatMap((base) => {
    const file = files.find((f) => f === `${base ?? ''}.ts` || f === `${base ?? ''}.tsx`);
    if (!file) return [];
    return [...src(file).matchAll(/^export (?:interface|type) (\w+)/gm)].map((m) => m[1] ?? '');
  });
  return [...values, ...types].filter(Boolean);
}

describe('README of the theme SDK', () => {
  it('exists', () => {
    expect(existsSync(readmePath)).toBe(true);
  });

  it('names every slot', () => {
    const names = slotNames();
    expect(names).toContain('Shell');
    expect(names).toContain('page:dashboard');
    expect(missing(names)).toEqual([]);
  });

  it('names every export, in particular every hook', () => {
    const names = exportNames();
    expect(names.filter((n) => n.startsWith('use')).length).toBeGreaterThan(5);
    expect(names).toContain('ThemeDefinition');
    expect(missing(names)).toEqual([]);
  });

  it('describes manifest fields and versioning', () => {
    const fields = [
      'id',
      'name',
      'version',
      'author',
      'sdkVersion',
      'colorScheme',
      'startColor',
      'preview',
      'slots',
      'permissions.network',
      'settingsSchema',
      'SDK_VERSION',
    ];
    expect(missing(fields)).toEqual([]);
    expect(readme()).toMatch(/semver/i);
  });
});
