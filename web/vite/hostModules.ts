import { createRequire } from 'node:module';
import type { Plugin } from 'vite';

// Bare module names the host gives theme modules through the import map, with their chunk names.
export const HOST_MODULES: Readonly<Record<string, string>> = {
  react: 'react',
  'react-dom': 'react-dom',
  'react-dom/client': 'react-dom-client',
  'react/jsx-runtime': 'jsx-runtime',
  '@nullradix/theme-sdk': 'theme-sdk',
};

// ESM packages need only `export *`; the others are CommonJS and need their export names.
const ESM_MODULES = new Set(['@nullradix/theme-sdk']);

const PREFIX = '\0nullradix-host:';
const require = createRequire(import.meta.url);

// Source of a re-export module; CommonJS export names are read at build time.
function reExport(spec: string): string {
  if (ESM_MODULES.has(spec)) return `export * from '${spec}';`;
  const names = Object.keys(require(spec) as object).filter(
    (name) => name !== 'default' && /^[A-Za-z_$][\w$]*$/.test(name),
  );
  return [
    `import * as m from '${spec}';`,
    'const d = m.default ?? m;',
    'export default d;',
    names.length > 0 ? `export const { ${names.join(', ')} } = d;` : '',
  ].join('\n');
}

// The re-export modules are virtual, for the build and the dev server alike.
const virtualModules = {
  resolveId(id: string) {
    return id.startsWith(PREFIX) ? id : null;
  },
  load(id: string) {
    return id.startsWith(PREFIX) ? reExport(id.slice(PREFIX.length)) : null;
  },
};

// The import map goes first in <head>, so it comes before every module script.
function importMapTag(imports: Record<string, string>) {
  return [
    {
      tag: 'script',
      attrs: { type: 'importmap' },
      children: JSON.stringify({ imports }),
      injectTo: 'head-prepend' as const,
    },
  ];
}

// Build: one re-export chunk per host module; dev server: the same modules under /@id/ addresses.
export function hostModules(): Plugin[] {
  const build: Plugin = {
    name: 'nullradix-host-modules',
    apply: 'build',
    ...virtualModules,
    buildStart() {
      for (const [spec, name] of Object.entries(HOST_MODULES)) {
        this.emitFile({
          type: 'chunk',
          id: PREFIX + spec,
          name: `host-${name}`,
          preserveSignature: 'strict',
        });
      }
    },
    transformIndexHtml: {
      order: 'post',
      handler(_html, context) {
        if (!context.bundle) return undefined;
        const imports: Record<string, string> = {};
        for (const file of Object.values(context.bundle)) {
          if (file.type === 'chunk' && file.facadeModuleId?.startsWith(PREFIX)) {
            imports[file.facadeModuleId.slice(PREFIX.length)] = `/${file.fileName}`;
          }
        }
        return importMapTag(imports);
      },
    },
  };

  const dev: Plugin = {
    name: 'nullradix-host-modules-dev',
    apply: 'serve',
    ...virtualModules,
    transformIndexHtml: {
      order: 'post',
      handler() {
        // Vite writes the null byte of virtual ids as __x00__ in addresses
        const imports = Object.fromEntries(
          Object.keys(HOST_MODULES).map((spec) => [spec, `/@id/__x00__nullradix-host:${spec}`]),
        );
        return importMapTag(imports);
      },
    },
  };

  return [build, dev];
}
