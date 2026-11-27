import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { FakeEventSource } from '../../test/eventSource';
import { renderApp } from '../../test/renderApp';
import { SNIPPETS_LINK_ID } from '../../shared/lib/snippets';
import type { Snippet } from './queries';

afterEach(() => {
  document.getElementById(SNIPPETS_LINK_ID)?.remove();
});

function item(overrides: Partial<Snippet> = {}): Snippet {
  return {
    id: 'red',
    name: 'Red links',
    css: 'a { color: red }',
    enabled: true,
    order: 1,
    createdAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

// In-memory daemon for the snippet routes.
function server(initial: Snippet[] = []) {
  let items = [...initial];
  const api = fakeApi({
    'GET /api/v1/snippets': () => json(200, { items }),
    'POST /api/v1/snippets': async (request) => {
      const body = (await request.json()) as { id: string; name: string; css: string };
      if (items.some((row) => row.id === body.id)) {
        return problem(409, 'snippet-exists', 'taken');
      }
      if (body.css.includes('@import')) {
        return problem(422, 'snippet-css-forbidden', 'the snippet is not allowed: @import');
      }
      const created = item({ ...body, order: items.length + 1 });
      items = [...items, created];
      return json(201, created);
    },
    'PATCH /api/v1/snippets/red': async (request) => {
      const body = (await request.json()) as Partial<Snippet>;
      const current = items.find((row) => row.id === 'red');
      if (!current) return problem(404, 'snippet-missing', 'gone');
      if (current.version !== body.version) return problem(409, 'version-conflict', 'stale');
      const next = { ...current, ...body, version: current.version + 1 };
      items = items.map((row) => (row.id === 'red' ? next : row));
      return json(200, next);
    },
    'DELETE /api/v1/snippets/red': (request) => {
      const version = Number(new URL(request.url).searchParams.get('version'));
      const current = items.find((row) => row.id === 'red');
      if (!current) return problem(404, 'snippet-missing', 'gone');
      if (current.version !== version) return problem(409, 'version-conflict', 'stale');
      items = items.filter((row) => row.id !== 'red');
      return new Response(null, { status: 204 });
    },
  });
  return {
    ...api,
    replace(next: Snippet[]) {
      items = next;
    },
  };
}

function list() {
  return screen.getByRole('list', { name: 'Snippets' });
}

describe('SnippetsSection', () => {
  it('says so when there are no snippets', async () => {
    server();
    renderApp('/settings');
    expect(await screen.findByText('No snippets yet.')).toBeInTheDocument();
  });

  it('lists snippets with their id and switch', async () => {
    server([item(), item({ id: 'blue', name: 'Blue', enabled: false, order: 2 })]);
    renderApp('/settings');
    const rows = within(list());
    expect(await rows.findByText('Red links')).toBeInTheDocument();
    expect(rows.getByText('red - On')).toBeInTheDocument();
    expect(rows.getByText('blue - Off')).toBeInTheDocument();
    expect(rows.getByRole('button', { name: 'Turn on Blue' })).toBeInTheDocument();
  });

  it('adds a snippet under an id made from its name and shows why a bad one is refused', async () => {
    const api = server([item()]);
    renderApp('/settings');
    await screen.findByText('Red links');
    const form = within(screen.getByRole('form', { name: 'New snippet' }));
    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'Red links');
    await userEvent.click(form.getByRole('textbox', { name: 'Style' }));
    await userEvent.paste('b { color: blue }');
    await userEvent.click(form.getByRole('button', { name: 'Add snippet' }));
    expect(await within(list()).findByText('red-links - On')).toBeInTheDocument();
    const [request] = api.requests('POST /api/v1/snippets');
    expect(await request?.clone().json()).toEqual({
      id: 'red-links',
      name: 'Red links',
      css: 'b { color: blue }',
    });
    expect(form.getByRole('textbox', { name: 'Name' })).toHaveValue('');

    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'Import');
    await userEvent.click(form.getByRole('textbox', { name: 'Style' }));
    await userEvent.paste("@import 'x.css';");
    await userEvent.click(form.getByRole('button', { name: 'Add snippet' }));
    expect(await form.findByRole('alert')).toHaveTextContent('the snippet is not allowed');
  });

  it('edits name and style and reports a stale version', async () => {
    const api = server([item()]);
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Edit Red links' }));
    const editor = screen.getByRole('form', { name: 'Edit Red links' });
    const text = within(editor).getByRole('textbox', { name: 'Style of Red links' });
    await userEvent.clear(text);
    await userEvent.click(text);
    await userEvent.paste('a { color: darkred }');
    await userEvent.click(within(editor).getByRole('button', { name: 'Save' }));
    await waitFor(() => {
      expect(screen.queryByRole('form', { name: 'Edit Red links' })).not.toBeInTheDocument();
    });
    const [request] = api.requests('PATCH /api/v1/snippets/red');
    expect(await request?.clone().json()).toEqual({ version: 1, css: 'a { color: darkred }' });

    api.set('PATCH /api/v1/snippets/red', () => problem(409, 'version-conflict', 'stale'));
    await userEvent.click(await screen.findByRole('button', { name: 'Edit Red links' }));
    const again = within(screen.getByRole('form', { name: 'Edit Red links' }));
    await userEvent.type(again.getByRole('textbox', { name: 'Style of Red links' }), '!');
    await userEvent.click(again.getByRole('button', { name: 'Save' }));
    expect(await again.findByRole('alert')).toHaveTextContent('changed in the meantime');
  });

  it('turns a snippet off and on', async () => {
    const api = server([item()]);
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Turn off Red links' }));
    expect(await screen.findByRole('button', { name: 'Turn on Red links' })).toBeInTheDocument();
    expect(screen.getByText('red - Off')).toBeInTheDocument();
    const [request] = api.requests('PATCH /api/v1/snippets/red');
    expect(await request?.clone().json()).toEqual({ version: 1, enabled: false });
  });

  it('deletes a snippet after asking', async () => {
    const api = server([item()]);
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Delete Red links' }));
    await userEvent.click(screen.getByRole('button', { name: 'Keep' }));
    expect(api.requests('DELETE /api/v1/snippets/red')).toHaveLength(0);
    await userEvent.click(screen.getByRole('button', { name: 'Delete Red links' }));
    await userEvent.click(
      within(screen.getByRole('group', { name: 'Delete Red links' })).getByRole('button', {
        name: 'Delete',
      }),
    );
    expect(await screen.findByText('No snippets yet.')).toBeInTheDocument();
    const [request] = api.requests('DELETE /api/v1/snippets/red');
    expect(new URL(request?.url ?? '').searchParams.get('version')).toBe('1');
  });
});

describe('active snippets in the app', () => {
  function sheet(): string | null {
    return document.getElementById(SNIPPETS_LINK_ID)?.getAttribute('href') ?? null;
  }

  it('links the stylesheet and follows changes announced by the daemon', async () => {
    const api = server([item()]);
    renderApp('/settings');
    await waitFor(() => {
      expect(sheet()).toBe('/api/v1/snippets/active.css?v=red.1');
    });
    await waitFor(() => {
      expect(FakeEventSource.instances).not.toHaveLength(0);
    });

    api.replace([item({ version: 2, css: 'a { color: green }' })]);
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        {
          seq: 2,
          entity: 'snippet',
          entityId: '00000000-0000-0000-0000-000000000001',
          op: 'update',
          at: 2,
        },
        '2',
      );
    });
    await waitFor(() => {
      expect(sheet()).toBe('/api/v1/snippets/active.css?v=red.2');
    });

    api.replace([item({ version: 3, enabled: false })]);
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        {
          seq: 3,
          entity: 'snippet',
          entityId: '00000000-0000-0000-0000-000000000001',
          op: 'update',
          at: 3,
        },
        '3',
      );
    });
    await waitFor(() => {
      expect(sheet()).toBeNull();
    });
  });
});
