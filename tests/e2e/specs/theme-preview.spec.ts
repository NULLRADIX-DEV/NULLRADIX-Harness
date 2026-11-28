import { createServer, type IncomingMessage, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import type { Page } from '@playwright/test';
import { expect, test } from '../support/fixtures';

// A 1x1 PNG.
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==',
  'base64',
);
const SVG = '<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>';
const SUM = 'ab'.repeat(32);

test.use({ daemonEnv: { NULLRADIX_THEME_CATALOG_LOCAL: 'true' } });

interface Source {
  origin: string;
  asked: IncomingMessage[];
  server: Server;
}

// A local index and local images, reachable in this mode.
async function startSource(): Promise<Source> {
  const asked: IncomingMessage[] = [];
  const server = createServer((request, response) => {
    asked.push(request);
    const origin = `http://127.0.0.1:${String((server.address() as AddressInfo).port)}`;
    const entry = (id: string, name: string, preview?: string) => ({
      id,
      name,
      version: '1.0.0',
      sdkVersion: '^2.0',
      downloadUrl: `${origin}/${id}.nullradix`,
      sha256: SUM,
      ...(preview ? { preview: `${origin}${preview}` } : {}),
    });
    if (request.url === '/index.json') {
      response.setHeader('content-type', 'application/json');
      response.end(
        JSON.stringify([
          entry('ocean', 'Ocean', '/ocean.png'),
          entry('cliff', 'Cliff', '/cliff.svg'),
          entry('plain', 'Plain'),
        ]),
      );
    } else if (request.url === '/ocean.png') {
      response.setHeader('content-type', 'image/png');
      response.end(PNG);
    } else if (request.url === '/cliff.svg') {
      response.setHeader('content-type', 'image/svg+xml');
      response.end(SVG);
    } else {
      response.statusCode = 404;
      response.end();
    }
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${String((server.address() as AddressInfo).port)}`;
  return { origin, asked, server };
}

test('the catalog shows previews fetched by the daemon and never loads the source itself', async ({
  page,
  browser,
  daemon,
}) => {
  const source = await startSource();
  try {
    const browserAsked: string[] = [];
    const watch = (watched: Page) => {
      watched.on('request', (request) => {
        if (request.url().startsWith(source.origin)) browserAsked.push(request.url());
      });
    };
    watch(page);
    await daemon.api('POST', '/api/v1/theme-catalog/sources', {
      url: `${source.origin}/index.json`,
    });
    await page.goto('/settings');

    const image = page.getByRole('img', { name: 'Preview of Ocean' });
    await expect(image).toBeVisible();
    await expect(image).toHaveAttribute(
      'src',
      /^\/api\/v1\/theme-catalog\/sources\/[0-9a-f]{12}\//,
    );
    // The image loads lazily and decodes only if the daemon sent a real one.
    await image.scrollIntoViewIfNeeded();
    await expect(image).toHaveJSProperty('naturalWidth', 1);

    // A bad image and an entry without one show the plain box.
    await expect(page.getByRole('img', { name: 'Preview of Cliff' })).toHaveCount(0);
    await expect(page.getByRole('img', { name: 'Preview of Plain' })).toHaveCount(0);

    // A fresh context avoids the immutable browser cache.
    const fresh = await browser.newContext({ baseURL: daemon.baseUrl });
    try {
      const again = await fresh.newPage();
      watch(again);
      await again.goto(await daemon.handoff());
      await again.getByRole('navigation', { name: 'Main navigation' }).waitFor();
      await again.goto('/settings');
      const second = again.getByRole('img', { name: 'Preview of Ocean' });
      await expect(second).toBeVisible();
      await expect(async () => {
        await second.scrollIntoViewIfNeeded();
        await expect(second).toHaveJSProperty('naturalWidth', 1);
      }).toPass();
    } finally {
      await fresh.close();
    }
    const paths = source.asked.map((request) => request.url ?? '');
    expect(paths.filter((path) => path === '/ocean.png')).toHaveLength(1);
    expect(paths).toContain('/cliff.svg');
    expect(
      source.asked.every((request) => request.headers['user-agent'] === 'nullradix-catalog'),
    ).toBe(true);
    expect(browserAsked).toEqual([]);
  } finally {
    source.server.close();
  }
});
