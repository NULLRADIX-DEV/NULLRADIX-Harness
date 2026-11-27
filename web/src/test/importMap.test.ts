// @vitest-environment node
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { SDK_VERSION } from '@nullradix/theme-sdk';
import { build } from 'vite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const WEB = resolve(import.meta.dirname, '..', '..');
const SPECS = [
  'react',
  'react-dom',
  'react-dom/client',
  'react/jsx-runtime',
  '@nullradix/theme-sdk',
];

let out = '';
let html = '';
let imports: Record<string, string> = {};
let mapText = '';

// The file of the build behind an address of the import map.
const fileOf = (url: string) => join(out, url.replace(/^\//, ''));

// Relative imports of a built script, followed all the way down.
function importsOf(file: string, seen = new Set<string>()): Set<string> {
  const code = readFileSync(join(out, 'assets', file), 'utf8');
  for (const match of code.matchAll(
    /from\s*["']\.\/([^"']+\.js)["']|import\s*["']\.\/([^"']+\.js)["']/g,
  )) {
    const dependency = match[1] ?? match[2];
    if (dependency !== undefined && !seen.has(dependency)) {
      seen.add(dependency);
      importsOf(dependency, seen);
    }
  }
  return seen;
}

beforeAll(async () => {
  out = mkdtempSync(join(tmpdir(), 'nullradix-importmap-'));
  await build({
    root: WEB,
    configFile: join(WEB, 'vite.config.ts'),
    logLevel: 'silent',
    build: { outDir: out, emptyOutDir: true },
  });
  html = readFileSync(join(out, 'index.html'), 'utf8');
  const map = /<script type="importmap">([\s\S]*?)<\/script>/.exec(html);
  mapText = map?.[1] ?? '';
  imports = map ? (JSON.parse(mapText) as { imports: Record<string, string> }).imports : {};
}, 120_000);

afterAll(() => {
  rmSync(out, { recursive: true, force: true });
});

describe('import map of the build', () => {
  it('stands once before every module script and names the five host modules', () => {
    expect(html.match(/<script type="importmap">/g)).toHaveLength(1);
    expect(html.indexOf('<script type="module"')).toBeGreaterThan(
      html.indexOf('<script type="importmap">'),
    );
    expect(Object.keys(imports).sort()).toEqual([...SPECS].sort());
    for (const url of Object.values(imports)) {
      expect(url).toMatch(/^\/assets\/.+\.js$/);
      expect(existsSync(fileOf(url))).toBe(true);
    }
  });

  it('is the only inline script, so the policy needs one hash for it', () => {
    const inline = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)].filter(
      (match) => !/\ssrc=/.test(match[1] ?? ''),
    );
    expect(inline).toHaveLength(1);
    expect(inline[0]?.[2]).toBe(mapText);
  });

  it('holds a single React copy that the app and the host modules share', () => {
    const assets = readdirSync(join(out, 'assets')).filter((file) => file.endsWith('.js'));
    // only the React core assigns its internals; react-dom and the re-exports just read them
    const marker = '__CLIENT_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE=';
    const reactFiles = assets.filter((file) =>
      readFileSync(join(out, 'assets', file), 'utf8').includes(marker),
    );
    expect(reactFiles).toHaveLength(1);
    const reactFile = reactFiles[0] ?? '';
    const entry = /<script type="module" crossorigin src="\/assets\/([^"]+)"/.exec(html)?.[1] ?? '';
    const reaches = (file: string) => file === reactFile || importsOf(file).has(reactFile);
    expect(reaches(entry)).toBe(true);
    for (const spec of ['react', 'react-dom', 'react-dom/client', 'react/jsx-runtime']) {
      expect(reaches((imports[spec] ?? '').replace('/assets/', ''))).toBe(true);
    }
  });

  it('gives a theme module the React instance and the SDK of the app', () => {
    const dir = mkdtempSync(join(tmpdir(), 'nullradix-probe-'));
    try {
      const mapped = Object.fromEntries(
        Object.entries(imports).map(([spec, url]) => [spec, pathToFileURL(fileOf(url)).href]),
      );
      // a resolve hook like the browser: bare names resolve only through the import map
      const hook = `resolve(spec, ctx, next) { const m = ${JSON.stringify(mapped)}; return spec in m ? { url: m[spec], shortCircuit: true } : next(spec, ctx); }`;
      writeFileSync(
        join(dir, 'theme-probe.mjs'),
        [
          "import React, { useState } from 'react';",
          "import { jsx } from 'react/jsx-runtime';",
          "import { createRoot } from 'react-dom/client';",
          "import * as sdk from '@nullradix/theme-sdk';",
          'export { React, useState, jsx, createRoot, sdk };',
        ].join('\n'),
      );
      writeFileSync(
        join(dir, 'run.mjs'),
        [
          "import { registerHooks } from 'node:module';",
          `registerHooks({ ${hook} });`,
          `const probe = await import(${JSON.stringify(pathToFileURL(join(dir, 'theme-probe.mjs')).href)});`,
          `const app = await import(${JSON.stringify(mapped['react'])});`,
          'console.log(JSON.stringify({',
          '  sameReact: probe.React === app.default,',
          '  sameHook: probe.useState === app.default.useState,',
          '  jsx: typeof probe.jsx, createRoot: typeof probe.createRoot, sdk: probe.sdk.SDK_VERSION,',
          '}));',
        ].join('\n'),
      );
      const stdout = execFileSync(process.execPath, [join(dir, 'run.mjs')], { encoding: 'utf8' });
      const result = JSON.parse(stdout.trim().split('\n').pop() ?? '{}') as unknown;
      expect(result).toEqual({
        sameReact: true,
        sameHook: true,
        jsx: 'function',
        createRoot: 'function',
        sdk: SDK_VERSION,
      });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }, 60_000);
});
