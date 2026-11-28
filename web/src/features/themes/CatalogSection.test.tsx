import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import type {
  CatalogSource,
  CatalogSourceStatus,
  CatalogTheme,
} from '../../shared/query/themeCatalog';
import { confirmation, fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';

const URL_A = 'https://themes.example.com/index.json';

function source(id: string, url: string): CatalogSource {
  return { id, url, addedAt: 1 };
}

function offered(overrides: Partial<CatalogTheme> = {}): CatalogTheme {
  return {
    sourceId: 'aaaaaaaaaaaa',
    id: 'night-blue',
    name: 'Night Blue',
    version: '1.2.0',
    sdkVersion: '^2.0',
    compatible: true,
    description: 'A dark theme',
    state: 'available',
    installedVersion: null,
    ...overrides,
  };
}

function status(overrides: Partial<CatalogSourceStatus> = {}): CatalogSourceStatus {
  return {
    source: source('aaaaaaaaaaaa', URL_A),
    problem: null,
    offered: 1,
    skipped: [],
    ...overrides,
  };
}

interface Options {
  sources?: CatalogSource[];
  themes?: CatalogTheme[];
  statuses?: CatalogSourceStatus[];
  addProblem?: Response;
  installProblem?: Response;
}

// In-memory daemon for the catalog routes.
function catalogServer(options: Options = {}) {
  let sources = options.sources ?? [];
  let version = sources.length;
  let themes = options.themes ?? [];
  const api = fakeApi({
    'GET /api/v1/themes': () => json(200, { items: [], active: 'nullradix' }),
    'GET /api/v1/theme-catalog/sources': () => json(200, { items: sources, version }),
    'GET /api/v1/theme-catalog/themes': () =>
      json(200, {
        items: themes,
        sources: options.statuses ?? sources.map((s) => status({ source: s })),
      }),
    'POST /api/v1/theme-catalog/sources': async (request) => {
      if (options.addProblem) return options.addProblem;
      const body = (await request.json()) as { url: string };
      const added = source('bbbbbbbbbbbb', body.url);
      sources = [...sources, added];
      version += 1;
      return json(201, added);
    },
    'DELETE /api/v1/theme-catalog/sources/aaaaaaaaaaaa': (request) => {
      const sent = Number(new URL(request.url).searchParams.get('version'));
      if (sent !== version) return problem(409, 'version-conflict', 'stale');
      sources = sources.filter((entry) => entry.id !== 'aaaaaaaaaaaa');
      version += 1;
      return new Response(null, { status: 204 });
    },
    'POST /api/v1/theme-catalog/sources/aaaaaaaaaaaa/themes/night-blue/install': () => {
      if (options.installProblem) return options.installProblem;
      themes = themes.map((theme) =>
        theme.id === 'night-blue'
          ? { ...theme, state: 'installed', installedVersion: '1.2.0' }
          : theme,
      );
      return json(201, {});
    },
  });
  return api;
}

describe('CatalogSection', () => {
  it('says so when there is no source and offers no theme list', async () => {
    const api = catalogServer();
    renderApp('/settings');
    expect(await screen.findByText(/No source yet/)).toBeInTheDocument();
    expect(screen.queryByText('Available themes')).not.toBeInTheDocument();
    expect(api.requests('GET /api/v1/theme-catalog/themes')).toHaveLength(0);
  });

  it('adds a source with the typed address and clears the field', async () => {
    const api = catalogServer();
    renderApp('/settings');
    const input = await screen.findByLabelText('Add a source');
    await userEvent.type(input, URL_A);
    await userEvent.click(screen.getByRole('button', { name: 'Add source' }));
    const list = screen.getByRole('list', { name: 'Catalog sources' });
    expect(await within(list).findByText(URL_A)).toBeInTheDocument();
    const [request] = api.requests('POST /api/v1/theme-catalog/sources');
    expect(await request?.json()).toEqual({ url: URL_A });
    expect(input).toHaveValue('');
  });

  it('shows why an address was refused and says when it is a repeat', async () => {
    catalogServer({
      addProblem: problem(422, 'catalog-source-invalid', 'the address must start with https://'),
    });
    renderApp('/settings');
    await userEvent.type(await screen.findByLabelText('Add a source'), 'http://x.example.com/i');
    await userEvent.click(screen.getByRole('button', { name: 'Add source' }));
    expect(await screen.findByText('the address must start with https://')).toBeInTheDocument();
  });

  it('knows a repeated address', async () => {
    catalogServer({ addProblem: problem(409, 'catalog-source-exists', 'exists') });
    renderApp('/settings');
    await userEvent.type(await screen.findByLabelText('Add a source'), URL_A);
    await userEvent.click(screen.getByRole('button', { name: 'Add source' }));
    expect(await screen.findByText('This address is already a source.')).toBeInTheDocument();
  });

  it('lists the offered themes with their state and installs one', async () => {
    const api = catalogServer({
      sources: [source('aaaaaaaaaaaa', URL_A)],
      themes: [
        offered(),
        offered({ id: 'old-gold', name: 'Old Gold', compatible: false, sdkVersion: '^9.0' }),
        offered({ id: 'sea', name: 'Sea', state: 'update', installedVersion: '1.0.0' }),
      ],
    });
    renderApp('/settings');
    const list = await screen.findByRole('list', { name: 'Available themes' });
    expect(within(list).getAllByText('v1.2.0 - Not installed')).toHaveLength(2);
    expect(within(list).getByText('v1.2.0 - Update from v1.0.0')).toBeInTheDocument();
    expect(within(list).getByText('Needs SDK ^9.0.')).toBeInTheDocument();
    expect(
      within(list).queryByRole('button', { name: 'Install Old Gold' }),
    ).not.toBeInTheDocument();
    expect(within(list).getByRole('button', { name: 'Update Sea' })).toBeInTheDocument();

    await userEvent.click(within(list).getByRole('button', { name: 'Install Night Blue' }));
    await waitFor(() => {
      expect(
        api.requests('POST /api/v1/theme-catalog/sources/aaaaaaaaaaaa/themes/night-blue/install'),
      ).toHaveLength(1);
    });
    await waitFor(() => {
      expect(within(list).getByText('v1.2.0 - Installed v1.2.0')).toBeInTheDocument();
    });
    expect(
      within(list).queryByRole('button', { name: 'Install Night Blue' }),
    ).not.toBeInTheDocument();
  });

  it('asks before replacing a theme installed from another source', async () => {
    const api = catalogServer({
      sources: [source('aaaaaaaaaaaa', URL_A)],
      themes: [offered({ state: 'other-source', installedVersion: '1.0.0' })],
    });
    renderApp('/settings');
    expect(await screen.findByText('v1.2.0 - Installed v1.0.0 from another source')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Replace Night Blue' }));
    const confirm = within(screen.getByRole('group', { name: 'Replace Night Blue' }));
    await userEvent.click(confirm.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('group', { name: 'Replace Night Blue' })).not.toBeInTheDocument();
    const path = 'POST /api/v1/theme-catalog/sources/aaaaaaaaaaaa/themes/night-blue/install';
    expect(api.requests(path)).toHaveLength(0);

    await userEvent.click(screen.getByRole('button', { name: 'Replace Night Blue' }));
    const again = within(screen.getByRole('group', { name: 'Replace Night Blue' }));
    await userEvent.click(again.getByRole('button', { name: 'Replace' }));
    await waitFor(() => {
      expect(api.requests(path)).toHaveLength(1);
    });
    expect(new URL(api.requests(path)[0]?.url ?? '').searchParams.get('replace')).toBe('true');
  });

  it('explains a failed install', async () => {
    catalogServer({
      sources: [source('aaaaaaaaaaaa', URL_A)],
      themes: [offered()],
      installProblem: problem(422, 'catalog-checksum-mismatch', 'mismatch'),
    });
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Install Night Blue' }));
    expect(
      await screen.findByText('The download does not match the catalog and was not installed.'),
    ).toBeInTheDocument();
  });

  it('reports a source that failed and the entries it skipped', async () => {
    catalogServer({
      sources: [source('aaaaaaaaaaaa', URL_A)],
      themes: [],
      statuses: [
        status({
          problem: { code: 'catalog-timeout', message: 'the source did not answer in time' },
          offered: 0,
          skipped: [{ index: 2, id: 'broken', reason: 'sha256 must be 64 hex characters' }],
        }),
      ],
    });
    renderApp('/settings');
    expect(
      await screen.findByText(`${URL_A}: the source did not answer in time.`),
    ).toBeInTheDocument();
    expect(screen.getByText(`1 entries of ${URL_A} were skipped`)).toBeInTheDocument();
    expect(screen.getByText(/#2 \(broken\): sha256 must be 64 hex characters/)).toBeInTheDocument();
  });

  it('shows only the themes and problems of sources that are still listed', async () => {
    catalogServer({
      sources: [source('aaaaaaaaaaaa', URL_A)],
      themes: [offered(), offered({ sourceId: 'cccccccccccc', id: 'ghost', name: 'Ghost' })],
      statuses: [
        status(),
        status({
          source: source('cccccccccccc', 'https://gone.example.com/index.json'),
          problem: { code: 'catalog-timeout', message: 'the source did not answer in time' },
        }),
      ],
    });
    renderApp('/settings');
    const list = within(await screen.findByRole('list', { name: 'Available themes' }));
    expect(list.getByText('Night Blue')).toBeInTheDocument();
    expect(list.queryByText('Ghost')).not.toBeInTheDocument();
    expect(screen.queryByText(/gone\.example\.com/)).not.toBeInTheDocument();
  });

  it('removes a source with the version of the list it showed', async () => {
    const api = catalogServer({ sources: [source('aaaaaaaaaaaa', URL_A)], themes: [] });
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: `Remove source ${URL_A}` }));
    await screen.findByText(/No source yet/);
    const [request] = api.requests('DELETE /api/v1/theme-catalog/sources/aaaaaaaaaaaa');
    expect(new URL(request?.url ?? '').searchParams.get('version')).toBe('1');
  });

  it('fetches the catalog again only on request', async () => {
    const api = catalogServer({ sources: [source('aaaaaaaaaaaa', URL_A)], themes: [offered()] });
    renderApp('/settings');
    await screen.findByRole('list', { name: 'Available themes' });
    expect(api.requests('GET /api/v1/theme-catalog/themes')).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await waitFor(() => {
      expect(api.requests('GET /api/v1/theme-catalog/themes')).toHaveLength(2);
    });
  });

  it('asks before installing a theme with code and installs it once confirmed', async () => {
    const route = 'POST /api/v1/theme-catalog/sources/aaaaaaaaaaaa/themes/night-blue/install';
    const api = catalogServer({ sources: [source('aaaaaaaaaaaa', URL_A)], themes: [offered()] });
    api.set(route, (request) => {
      if (new URL(request.url).searchParams.get('confirmedSha256') === 'abc123')
        return json(201, {});
      return confirmation({ hasCode: true, sha256: 'abc123', domains: ['cdn.example.com'] });
    });
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Install Night Blue' }));
    const dialog = await screen.findByRole('alertdialog', { name: 'Install Night Blue 1.0.0?' });
    expect(within(dialog).getByText(/This package contains code/)).toBeInTheDocument();
    expect(within(dialog).getByText('cdn.example.com')).toBeInTheDocument();
    expect(screen.queryByText('the package needs a confirmation')).not.toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole('button', { name: 'Install' }));
    await waitFor(() => {
      expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    });
    const urls = api.requests(route).map((request) => new URL(request.url).search);
    expect(urls).toEqual(['', '?confirmedSha256=abc123']);
  });

  it('installs nothing when the confirmation is cancelled', async () => {
    const route = 'POST /api/v1/theme-catalog/sources/aaaaaaaaaaaa/themes/night-blue/install';
    const api = catalogServer({
      sources: [source('aaaaaaaaaaaa', URL_A)],
      themes: [offered({ state: 'update', installedVersion: '1.0.0' })],
    });
    api.set(route, () =>
      confirmation({
        update: true,
        hasCode: true,
        added: { code: true, claudeTheme: false, domains: [] },
      }),
    );
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Update Night Blue' }));
    const dialog = await screen.findByRole('alertdialog', { name: 'Update Night Blue 1.0.0?' });
    expect(within(dialog).getByText(/new in this update/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(api.requests(route)).toHaveLength(1);
  });
});

describe('CatalogSection previews', () => {
  const PROXY =
    '/api/v1/theme-catalog/sources/aaaaaaaaaaaa/themes/night-blue/preview?v=0123456789abcdef';

  async function listed(themes: CatalogTheme[]) {
    catalogServer({ sources: [source('aaaaaaaaaaaa', URL_A)], themes });
    renderApp('/settings');
    return within(await screen.findByRole('list', { name: 'Available themes' }));
  }

  it('shows the image through the proxy path of the daemon', async () => {
    const list = await listed([offered({ previewUrl: PROXY })]);
    const image = list.getByRole('img', { name: 'Preview of Night Blue' });
    expect(image).toHaveAttribute('src', PROXY);
    expect(image).toHaveAttribute('loading', 'lazy');
  });

  it('shows a box with the first letter when the entry has no preview', async () => {
    const list = await listed([offered({ previewUrl: null }), offered({ id: 'sea', name: 'Sea' })]);
    expect(list.queryByRole('img')).not.toBeInTheDocument();
    expect(list.getByText('N')).toBeInTheDocument();
    expect(list.getByText('S')).toBeInTheDocument();
  });

  it('shows the box instead when the image fails, and keeps the rest of the row', async () => {
    const list = await listed([offered({ previewUrl: PROXY })]);
    fireEvent.error(list.getByRole('img', { name: 'Preview of Night Blue' }));
    expect(list.queryByRole('img')).not.toBeInTheDocument();
    expect(list.getByText('N')).toBeInTheDocument();
    expect(list.getByRole('button', { name: 'Install Night Blue' })).toBeInTheDocument();
  });

  it('tries a failed image again when the list is fetched again', async () => {
    const api = catalogServer({
      sources: [source('aaaaaaaaaaaa', URL_A)],
      themes: [offered({ previewUrl: PROXY })],
    });
    renderApp('/settings');
    const list = within(await screen.findByRole('list', { name: 'Available themes' }));
    fireEvent.error(list.getByRole('img', { name: 'Preview of Night Blue' }));
    expect(list.queryByRole('img')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await waitFor(() => {
      expect(api.requests('GET /api/v1/theme-catalog/themes')).toHaveLength(2);
    });
    expect(await list.findByRole('img', { name: 'Preview of Night Blue' })).toBeInTheDocument();
  });

  it('never puts an address that is not the proxy path of a source into an image', async () => {
    const odd = [
      'https://evil.example/preview.png',
      'http://127.0.0.1/preview.png',
      '//evil.example/preview.png',
      'data:image/png;base64,AAAA',
      'javascript:alert(1)',
      '/api/v1/themes/night-blue/files/preview.png',
      '/api/v1/theme-catalog/sources/aaaaaaaaaaaa/themes/night-blue/preview?v=1&x=https://evil.example',
      '/api/v1/theme-catalog/sources/aaaaaaaaaaaa/themes/../preview',
      `${PROXY}#x`,
      ` ${PROXY}`,
    ];
    const list = await listed(
      odd.map((previewUrl, number) =>
        offered({ id: `theme-${String(number)}`, name: `Theme ${String(number)}`, previewUrl }),
      ),
    );
    expect(list.queryByRole('img')).not.toBeInTheDocument();
    expect(document.querySelectorAll('img')).toHaveLength(0);
  });

  it('asks for no remote address, whatever the catalog holds', async () => {
    const api = catalogServer({
      sources: [source('aaaaaaaaaaaa', URL_A)],
      themes: [offered({ previewUrl: PROXY })],
    });
    renderApp('/settings');
    await screen.findByRole('img', { name: 'Preview of Night Blue' });
    for (const image of document.querySelectorAll('img')) {
      expect(new URL(image.src).origin).toBe(window.location.origin);
    }
    const asked = (api.fetch.mock.calls as [Request][]).map(([request]) => new URL(request.url));
    expect(asked.every((url) => url.origin === window.location.origin)).toBe(true);
  });
});
