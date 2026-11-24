import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { FakeEventSource } from '../../test/eventSource';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { CONFLICT_MESSAGE } from './ProjectsSection';
import { projectKeys, type Project } from './queries';

const ARC_ID = '0190a0b0-0000-7000-8000-000000000001';

function project(overrides: Partial<Project> = {}): Project {
  return {
    id: ARC_ID,
    key: 'ARC',
    name: 'Arches',
    createdAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

// In-memory daemon for the projects routes.
function projectsServer(initial: Project[] = [project()]) {
  let items = [...initial];
  const api = fakeApi({
    'GET /api/v1/projects': () => json(200, { items }),
    'POST /api/v1/projects': async (request) => {
      const body = (await request.json()) as { key: string; name: string };
      const created = project({
        id: `0190a0b0-0000-7000-8000-00000000000${String(items.length + 2)}`,
        ...body,
      });
      items = [...items, created];
      return json(201, created);
    },
    [`PATCH /api/v1/projects/${ARC_ID}`]: async (request) => {
      const body = (await request.json()) as { name: string; version: number };
      const current = items.find((item) => item.id === ARC_ID);
      if (!current) return problem(404, 'project-not-found', 'no such project');
      if (current.version !== body.version) return problem(409, 'version-conflict', 'stale');
      const renamed = { ...current, name: body.name, version: current.version + 1 };
      items = items.map((item) => (item.id === ARC_ID ? renamed : item));
      return json(200, renamed);
    },
    [`DELETE /api/v1/projects/${ARC_ID}`]: (request) => {
      const version = Number(new URL(request.url).searchParams.get('version'));
      const current = items.find((item) => item.id === ARC_ID);
      if (!current) return problem(404, 'project-not-found', 'no such project');
      if (current.version !== version) return problem(409, 'version-conflict', 'stale');
      items = items.filter((item) => item.id !== ARC_ID);
      return new Response(null, { status: 204 });
    },
  });
  return {
    api,
    // Simulates a change made by another client.
    bumpElsewhere(name: string) {
      items = items.map((item) => ({ ...item, name, version: item.version + 1 }));
    },
  };
}

function projectList() {
  return screen.findByRole('list', { name: 'Projects' });
}

function createForm() {
  return within(screen.getByRole('form', { name: 'New project' }));
}

describe('ProjectsSection', () => {
  it('lists projects with key and name next to the theme selector', async () => {
    projectsServer();
    renderApp('/settings');
    const list = await projectList();
    expect(within(list).getByText('ARC')).toBeInTheDocument();
    expect(within(list).getByText('Arches')).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Theme' })).toBeInTheDocument();
  });

  it('shows an empty state without projects', async () => {
    projectsServer([]);
    renderApp('/settings');
    expect(await screen.findByText('No projects yet.')).toBeInTheDocument();
  });

  it('creates a project with an upper-case key', async () => {
    const { api } = projectsServer([]);
    renderApp('/settings');
    await screen.findByText('No projects yet.');
    const form = createForm();
    await userEvent.type(form.getByRole('textbox', { name: 'Key' }), 'nx1');
    expect(form.getByRole('textbox', { name: 'Key' })).toHaveValue('NX1');
    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'Nexus');
    await userEvent.click(form.getByRole('button', { name: 'Create project' }));

    const list = await projectList();
    expect(within(list).getByText('NX1')).toBeInTheDocument();
    expect(within(list).getByText('Nexus')).toBeInTheDocument();
    const [request] = api.requests('POST /api/v1/projects');
    expect(await request?.clone().json()).toEqual({ key: 'NX1', name: 'Nexus' });
    expect(form.getByRole('textbox', { name: 'Key' })).toHaveValue('');
  });

  it('shows the problem detail of a validation error', async () => {
    const { api } = projectsServer([]);
    api.set('POST /api/v1/projects', () =>
      problem(422, 'invalid-project-key', 'Project keys are 2 to 6 characters'),
    );
    renderApp('/settings');
    await screen.findByText('No projects yet.');
    const form = createForm();
    await userEvent.type(form.getByRole('textbox', { name: 'Key' }), '1A');
    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'Broken');
    await userEvent.click(form.getByRole('button', { name: 'Create project' }));

    expect(await form.findByRole('alert')).toHaveTextContent('Project keys are 2 to 6 characters');
    expect(form.getByRole('textbox', { name: 'Key' })).toHaveValue('1A');
  });

  it('shows the detail when the key is taken', async () => {
    const { api } = projectsServer();
    api.set('POST /api/v1/projects', () => problem(409, 'project-key-taken', 'ARC is taken'));
    renderApp('/settings');
    await projectList();
    const form = createForm();
    await userEvent.type(form.getByRole('textbox', { name: 'Key' }), 'ARC');
    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'Again');
    await userEvent.click(form.getByRole('button', { name: 'Create project' }));
    expect(await form.findByRole('alert')).toHaveTextContent('ARC is taken');
  });

  it('renames a project with its version', async () => {
    const { api } = projectsServer();
    renderApp('/settings');
    await projectList();
    await userEvent.click(screen.getByRole('button', { name: 'Rename ARC' }));
    const form = within(screen.getByRole('form', { name: 'Rename ARC' }));
    const input = form.getByRole('textbox', { name: 'Name' });
    await userEvent.clear(input);
    await userEvent.type(input, 'Arches 2');
    await userEvent.click(form.getByRole('button', { name: 'Save' }));

    expect(await screen.findByText('Arches 2')).toBeInTheDocument();
    expect(screen.queryByRole('form', { name: 'Rename ARC' })).not.toBeInTheDocument();
    const [request] = api.requests(`PATCH /api/v1/projects/${ARC_ID}`);
    expect(await request?.clone().json()).toEqual({ name: 'Arches 2', version: 1 });
  });

  it('reports a rename conflict, refetches and succeeds on retry', async () => {
    const server = projectsServer();
    renderApp('/settings');
    await projectList();
    const listCalls = () => server.api.requests('GET /api/v1/projects').length;
    await userEvent.click(screen.getByRole('button', { name: 'Rename ARC' }));
    server.bumpElsewhere('Arches elsewhere');
    const before = listCalls();

    const form = within(screen.getByRole('form', { name: 'Rename ARC' }));
    const input = form.getByRole('textbox', { name: 'Name' });
    await userEvent.clear(input);
    await userEvent.type(input, 'Mine');
    await userEvent.click(form.getByRole('button', { name: 'Save' }));

    expect(await form.findByRole('alert')).toHaveTextContent(CONFLICT_MESSAGE);
    await waitFor(() => {
      expect(listCalls()).toBeGreaterThan(before);
    });
    expect(await screen.findByText('Arches elsewhere')).toBeInTheDocument();

    await userEvent.click(form.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Mine')).toBeInTheDocument();
    const patches = server.api.requests(`PATCH /api/v1/projects/${ARC_ID}`);
    expect(await patches.at(-1)?.clone().json()).toEqual({ name: 'Mine', version: 2 });
  });

  it('deletes a project after confirmation', async () => {
    const { api } = projectsServer();
    renderApp('/settings');
    await projectList();
    await userEvent.click(screen.getByRole('button', { name: 'Delete ARC' }));
    const confirm = within(screen.getByRole('group', { name: 'Delete ARC' }));
    expect(confirm.getByText(/This cannot be undone/)).toBeInTheDocument();

    await userEvent.click(confirm.getByRole('button', { name: 'Cancel' }));
    expect(api.requests(`DELETE /api/v1/projects/${ARC_ID}`)).toHaveLength(0);

    await userEvent.click(screen.getByRole('button', { name: 'Delete ARC' }));
    await userEvent.click(
      within(screen.getByRole('group', { name: 'Delete ARC' })).getByRole('button', {
        name: 'Delete',
      }),
    );
    expect(await screen.findByText('No projects yet.')).toBeInTheDocument();
    const [request] = api.requests(`DELETE /api/v1/projects/${ARC_ID}`);
    expect(new URL(request?.url ?? '').search).toBe('?version=1');
  });

  it('reports a delete conflict', async () => {
    const server = projectsServer();
    renderApp('/settings');
    await projectList();
    await userEvent.click(screen.getByRole('button', { name: 'Delete ARC' }));
    server.bumpElsewhere('Arches elsewhere');
    const confirm = within(screen.getByRole('group', { name: 'Delete ARC' }));
    await userEvent.click(confirm.getByRole('button', { name: 'Delete' }));
    expect(await confirm.findByRole('alert')).toHaveTextContent(CONFLICT_MESSAGE);
    expect(await confirm.findByText('Arches elsewhere')).toBeInTheDocument();
  });

  it('refetches the list when the change stream reports a project change', async () => {
    const server = projectsServer();
    renderApp('/settings');
    await projectList();
    await waitFor(() => {
      expect(FakeEventSource.instances).not.toHaveLength(0);
    });
    server.bumpElsewhere('Arches live');
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        { seq: 3, entity: 'project', entityId: ARC_ID, op: 'update', at: 2 },
        '3',
      );
    });
    expect(await screen.findByText('Arches live')).toBeInTheDocument();
    expect(projectKeys.list()).toEqual(['projects', 'list']);
  });

  it('only saves a changed, non-empty name', async () => {
    projectsServer();
    renderApp('/settings');
    await projectList();
    await userEvent.click(screen.getByRole('button', { name: 'Rename ARC' }));
    const form = within(screen.getByRole('form', { name: 'Rename ARC' }));
    const input = form.getByRole('textbox', { name: 'Name' });
    const save = form.getByRole('button', { name: 'Save' });
    expect(input).toHaveFocus();
    expect(save).toBeDisabled();

    await userEvent.clear(input);
    await userEvent.type(input, '   ');
    expect(save).toBeDisabled();

    await userEvent.type(input, 'New');
    expect(save).toBeEnabled();
  });

  it('returns focus to the rename button after escape and after saving', async () => {
    projectsServer();
    renderApp('/settings');
    await projectList();
    const trigger = screen.getByRole('button', { name: 'Rename ARC' });
    await userEvent.click(trigger);
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('form', { name: 'Rename ARC' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Rename ARC' })).toHaveFocus();

    await userEvent.click(screen.getByRole('button', { name: 'Rename ARC' }));
    await userEvent.keyboard('{Control>}a{/Control}Renamed{Enter}');
    expect(await screen.findByText('Renamed')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Rename ARC' })).toHaveFocus();
  });

  it('focuses cancel in the delete confirmation and returns focus on escape or cancel', async () => {
    const { api } = projectsServer();
    renderApp('/settings');
    await projectList();
    await userEvent.click(screen.getByRole('button', { name: 'Delete ARC' }));
    const confirm = within(screen.getByRole('group', { name: 'Delete ARC' }));
    expect(confirm.getByRole('button', { name: 'Cancel' })).toHaveFocus();

    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('group', { name: 'Delete ARC' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete ARC' })).toHaveFocus();

    await userEvent.keyboard('{Enter}');
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    expect(screen.getByRole('button', { name: 'Delete ARC' })).toHaveFocus();
    expect(api.requests(`DELETE /api/v1/projects/${ARC_ID}`)).toHaveLength(0);
  });
});
