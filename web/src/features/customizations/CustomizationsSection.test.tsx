import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { Customization } from './queries';

const SKILL = '---\ndescription: Reviews\n---\nBody';

function item(overrides: Partial<Customization> = {}): Customization {
  return {
    point: 'skill',
    name: 'review',
    kind: 'new',
    state: 'active',
    content: SKILL,
    enabled: true,
    base: 'none',
    standard: false,
    problems: [],
    warnings: [],
    createdAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

// In-memory daemon for the customization routes.
function server(initial: Customization[] = []) {
  let items = [...initial];
  const api = fakeApi({
    'GET /api/v1/customizations': () => json(200, { items }),
    'POST /api/v1/customizations': async (request) => {
      const body = (await request.json()) as {
        point: Customization['point'];
        name: string;
        content?: string;
      };
      if (items.some((row) => row.name === body.name && row.point === body.point)) {
        return problem(409, 'customization-exists', 'already customized');
      }
      if (body.content === undefined) {
        return problem(422, 'customization-no-standard', 'nothing shipped');
      }
      const created = item({ point: body.point, name: body.name, content: body.content });
      items = [...items, created];
      return json(201, created);
    },
    'PATCH /api/v1/customizations/skill/review': async (request) => {
      const body = (await request.json()) as {
        version: number;
        content?: string;
        enabled?: boolean;
      };
      const current = items.find((row) => row.name === 'review');
      if (!current) return problem(404, 'customization-missing', 'gone');
      if (current.version !== body.version) return problem(409, 'version-conflict', 'stale');
      const next = {
        ...current,
        ...(body.content !== undefined && { content: body.content }),
        ...(body.enabled !== undefined && {
          enabled: body.enabled,
          state: body.enabled ? ('active' as const) : ('disabled' as const),
        }),
        version: current.version + 1,
      };
      items = items.map((row) => (row.name === 'review' ? next : row));
      return json(200, next);
    },
    'DELETE /api/v1/customizations/skill/review': (request) => {
      const version = Number(new URL(request.url).searchParams.get('version'));
      const current = items.find((row) => row.name === 'review');
      if (!current) return problem(404, 'customization-missing', 'gone');
      if (current.version !== version) return problem(409, 'version-conflict', 'stale');
      items = items.filter((row) => row.name !== 'review');
      return new Response(null, { status: 204 });
    },
  });
  return api;
}

function list() {
  return screen.getByRole('list', { name: 'Customizations' });
}

describe('CustomizationsSection', () => {
  it('says so when nothing is customized', async () => {
    server();
    renderApp('/settings');
    expect(await screen.findByText('Nothing is customized yet.')).toBeInTheDocument();
  });

  it('lists customizations with kind, state, problems and hints', async () => {
    server([
      item(),
      item({
        point: 'output-style',
        name: 'terse',
        kind: 'replace',
        state: 'fallback',
        problems: [{ code: 'description-missing', message: 'the head has no description' }],
        warnings: [{ code: 'base-outdated', message: 'the shipped file changed' }],
      }),
    ]);
    renderApp('/settings');
    const rows = within(list());
    expect(await rows.findByText('review')).toBeInTheDocument();
    expect(rows.getByText(/Skill - new - Active/)).toBeInTheDocument();
    expect(
      rows.getByText(/Output style - replaces the shipped one - Shipped file in charge/),
    ).toBeInTheDocument();
    expect(rows.getByText('the head has no description')).toBeInTheDocument();
    expect(rows.getByText('the shipped file changed')).toBeInTheDocument();
  });

  it('adds a customization and shows why a bad one is refused', async () => {
    const api = server();
    renderApp('/settings');
    await screen.findByText('Nothing is customized yet.');
    const form = within(screen.getByRole('form', { name: 'New customization' }));
    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'review');
    await userEvent.type(form.getByRole('textbox', { name: 'Text' }), 'text');
    await userEvent.click(form.getByRole('button', { name: 'Add customization' }));
    expect(await within(list()).findByText('review')).toBeInTheDocument();
    const [request] = api.requests('POST /api/v1/customizations');
    expect(await request?.clone().json()).toEqual({
      point: 'skill',
      name: 'review',
      content: 'text',
    });
    expect(form.getByRole('textbox', { name: 'Name' })).toHaveValue('');

    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'other');
    await userEvent.click(form.getByRole('button', { name: 'Add customization' }));
    expect(await form.findByRole('alert')).toHaveTextContent('Nothing is shipped under this name');
  });

  it('edits the text and reports a stale version', async () => {
    const api = server([item()]);
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Edit review' }));
    const editor = screen.getByRole('form', { name: 'Edit review' });
    const text = within(editor).getByRole('textbox', { name: 'Text of review' });
    await userEvent.clear(text);
    await userEvent.type(text, 'new text');
    await userEvent.click(within(editor).getByRole('button', { name: 'Save' }));
    await waitFor(() => {
      expect(screen.queryByRole('form', { name: 'Edit review' })).not.toBeInTheDocument();
    });
    const [request] = api.requests('PATCH /api/v1/customizations/skill/review');
    expect(await request?.clone().json()).toEqual({ version: 1, content: 'new text' });

    api.set('PATCH /api/v1/customizations/skill/review', () =>
      problem(409, 'version-conflict', 'stale'),
    );
    await userEvent.click(await screen.findByRole('button', { name: 'Edit review' }));
    const again = within(screen.getByRole('form', { name: 'Edit review' }));
    await userEvent.type(again.getByRole('textbox', { name: 'Text of review' }), '!');
    await userEvent.click(again.getByRole('button', { name: 'Save' }));
    expect(await again.findByRole('alert')).toHaveTextContent('changed in the meantime');
  });

  it('switches a customization off and on', async () => {
    const api = server([item()]);
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Disable review' }));
    expect(await screen.findByRole('button', { name: 'Enable review' })).toBeInTheDocument();
    expect(screen.getByText(/Disabled/)).toBeInTheDocument();
    const [request] = api.requests('PATCH /api/v1/customizations/skill/review');
    expect(await request?.clone().json()).toEqual({ version: 1, enabled: false });
  });

  it('drops a customization after asking', async () => {
    const api = server([item()]);
    renderApp('/settings');
    await userEvent.click(await screen.findByRole('button', { name: 'Reset review' }));
    await userEvent.click(screen.getByRole('button', { name: 'Keep' }));
    expect(api.requests('DELETE /api/v1/customizations/skill/review')).toHaveLength(0);
    await userEvent.click(screen.getByRole('button', { name: 'Reset review' }));
    await userEvent.click(
      within(screen.getByRole('group', { name: 'Reset review' })).getByRole('button', {
        name: 'Drop',
      }),
    );
    expect(await screen.findByText('Nothing is customized yet.')).toBeInTheDocument();
    const [request] = api.requests('DELETE /api/v1/customizations/skill/review');
    expect(new URL(request?.url ?? '').searchParams.get('version')).toBe('1');
  });
});
