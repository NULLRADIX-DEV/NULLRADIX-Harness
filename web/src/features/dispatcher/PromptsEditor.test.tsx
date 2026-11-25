import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { configView } from '../../test/dispatcherFixture';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { PROMPT_CONFLICT_MESSAGE } from './PromptsEditor';
import type { PromptDto, PromptKind, PromptPreview } from './queries';

const DEFAULTS: Record<PromptKind, string> = {
  work: 'Work {{ticketId}} as {{agent}}',
  plan: 'Plan {{ticketId}} as {{agent}}',
  research: 'Research {{questionId}}',
  merge: 'Merge {{branch}}',
};

const LABELS: Record<PromptKind, string> = {
  work: 'Work',
  plan: 'Planning',
  research: 'Research',
  merge: 'Merge',
};

function prompt(kind: PromptKind, template: string | null = null): PromptDto {
  return {
    kind,
    label: LABELS[kind],
    default: DEFAULTS[kind],
    template,
    placeholders: [
      { name: 'agent', description: 'Name of the agent' },
      { name: 'ticketId', description: 'Key of the ticket' },
    ],
    required: ['Placeholder {{ticketId}}', 'Placeholder {{agent}}'],
  };
}

interface Sent {
  kind: string;
  body: Record<string, unknown> | null;
  query: string;
}

// In-memory daemon for the prompt routes; own templates by kind.
function promptServer(own: Partial<Record<PromptKind, string>> = {}) {
  let version = 1;
  const templates: Partial<Record<PromptKind, string>> = { ...own };
  const puts: Sent[] = [];
  const deletes: Sent[] = [];
  const previews: Record<string, unknown>[] = [];
  let preview: PromptPreview = {
    kind: 'work',
    key: null,
    sample: true,
    prompt: 'RENDERED',
    standIns: [],
  };
  let refuse: Response | null = null;
  const kinds: PromptKind[] = ['work', 'plan', 'research', 'merge'];
  const detail = (kind: PromptKind) => ({
    prompt: prompt(kind, templates[kind] ?? null),
    version,
  });
  const kindOf = (request: Request) => new URL(request.url).pathname.split('/').pop() as PromptKind;
  const api = fakeApi({
    'GET /api/v1/dispatcher/config': () => json(200, configView()),
    'GET /api/v1/accounts': () => json(200, { items: [], accountsDir: '/data/accounts' }),
    'GET /api/v1/dispatcher/prompts': () =>
      json(200, { items: kinds.map((kind) => detail(kind).prompt), version }),
    'PUT /api/v1/dispatcher/prompts/plan': async (request) => {
      const body = (await request.json()) as { template: string; version: number };
      puts.push({ kind: kindOf(request), body, query: '' });
      if (refuse) {
        const answer = refuse;
        refuse = null;
        return answer;
      }
      if (body.version !== version) return problem(409, 'version-conflict', 'stale');
      templates.plan = body.template;
      version += 1;
      return json(200, detail('plan'));
    },
    'DELETE /api/v1/dispatcher/prompts/plan': (request) => {
      const query = new URL(request.url).search;
      deletes.push({ kind: kindOf(request), body: null, query });
      if (query !== `?version=${String(version)}`) return problem(409, 'version-conflict', 'stale');
      delete templates.plan;
      version += 1;
      return json(200, detail('plan'));
    },
    'POST /api/v1/dispatcher/prompt-previews': async (request) => {
      previews.push((await request.json()) as Record<string, unknown>);
      if (refuse) {
        const answer = refuse;
        refuse = null;
        return answer;
      }
      return json(200, preview);
    },
  });
  return {
    api,
    puts,
    deletes,
    previews,
    templates,
    answerPreview(next: Partial<PromptPreview>) {
      preview = { ...preview, ...next };
    },
    refuseNext(response: Response) {
      refuse = response;
    },
    bump() {
      version += 1;
    },
  };
}

function refused(messages: string[]): Response {
  return new Response(
    JSON.stringify({
      type: 'about:blank',
      title: 'Unprocessable Entity',
      status: 422,
      code: 'dispatcher-prompt-invalid',
      detail: 'the prompt template has unknown placeholders or lacks required parts',
      errors: messages.map((message) => ({ field: 'template', message })),
    }),
    { status: 422, headers: { 'content-type': 'application/problem+json' } },
  );
}

async function open() {
  renderApp('/settings');
  return screen.findByRole('region', { name: 'Prompts' });
}

const area = (section: HTMLElement, label: string) =>
  within(section).getByRole<HTMLTextAreaElement>('textbox', { name: label });

describe('PromptsEditor', () => {
  it('shows the default of a kind with its placeholders and required parts', async () => {
    promptServer({ plan: 'Own plan {{ticketId}} {{agent}}' });
    const section = await open();
    expect(area(section, 'Work template').value).toBe(DEFAULTS.work);
    expect(within(section).getByRole('status')).toHaveTextContent('Using the default template.');
    expect(
      within(within(section).getByRole('list', { name: 'Placeholders' })).getAllByRole('listitem'),
    ).toHaveLength(2);
    expect(within(section).getByText('Placeholder {{agent}}')).toBeInTheDocument();
    expect(within(section).getByRole('button', { name: 'Planning (own)' })).toBeInTheDocument();
    expect(within(section).getByRole('button', { name: 'Save template' })).toBeDisabled();
    expect(within(section).getByRole('button', { name: 'Reset to default' })).toBeDisabled();

    await userEvent.click(within(section).getByRole('button', { name: 'Planning (own)' }));
    expect(area(section, 'Planning template').value).toBe('Own plan {{ticketId}} {{agent}}');
    expect(within(section).getByRole('status')).toHaveTextContent('Using an own template.');
    expect(within(section).getByRole('button', { name: 'Reset to default' })).toBeEnabled();
  });

  it('saves an edited template on the version it was based on', async () => {
    const server = promptServer();
    const section = await open();
    await userEvent.click(within(section).getByRole('button', { name: 'Planning' }));
    const text = area(section, 'Planning template');
    await userEvent.type(text, ' extra');
    expect(within(section).getByRole('status')).toHaveTextContent('Unsaved changes.');
    await userEvent.click(within(section).getByRole('button', { name: 'Save template' }));
    await waitFor(() => {
      expect(within(section).getByRole('status')).toHaveTextContent('Saved.');
    });
    expect(server.puts).toEqual([
      {
        kind: 'plan',
        body: { template: 'Plan {{ticketId}} as {{agent}} extra', version: 1 },
        query: '',
      },
    ]);
    expect(within(section).getByRole('status')).toHaveTextContent('Using an own template.');
    expect(within(section).getByRole('button', { name: 'Reset to default' })).toBeEnabled();
  });

  it('lists every problem the daemon finds and keeps the text', async () => {
    const server = promptServer();
    const section = await open();
    await userEvent.click(within(section).getByRole('button', { name: 'Planning' }));
    await userEvent.click(area(section, 'Planning template'));
    await userEvent.paste(' {{nope}}');
    server.refuseNext(
      refused(['uses unknown placeholders {{nope}}', 'lacks required parts: File plan.json']),
    );
    await userEvent.click(within(section).getByRole('button', { name: 'Save template' }));
    const list = await within(section).findByRole('list', { name: 'Template problems' });
    expect(
      within(list)
        .getAllByRole('alert')
        .map((item) => item.textContent),
    ).toEqual(['uses unknown placeholders {{nope}}', 'lacks required parts: File plan.json']);
    expect(area(section, 'Planning template').value).toContain('{{nope}}');
    expect(server.templates.plan).toBeUndefined();
  });

  it('goes back to the default with the reset button', async () => {
    const server = promptServer({ plan: 'Own plan {{ticketId}} {{agent}}' });
    const section = await open();
    await userEvent.click(within(section).getByRole('button', { name: 'Planning (own)' }));
    await userEvent.click(within(section).getByRole('button', { name: 'Reset to default' }));
    await waitFor(() => {
      expect(area(section, 'Planning template').value).toBe(DEFAULTS.plan);
    });
    expect(server.deletes).toEqual([{ kind: 'plan', body: null, query: '?version=1' }]);
    expect(within(section).getByRole('status')).toHaveTextContent('Using the default template.');
  });

  it('treats the default text pasted over an own template as a reset', async () => {
    const server = promptServer({ plan: 'Own plan {{ticketId}} {{agent}}' });
    const section = await open();
    await userEvent.click(within(section).getByRole('button', { name: 'Planning (own)' }));
    const text = area(section, 'Planning template');
    await userEvent.clear(text);
    await userEvent.click(text);
    await userEvent.paste(DEFAULTS.plan);
    await userEvent.click(within(section).getByRole('button', { name: 'Save template' }));
    await waitFor(() => {
      expect(server.deletes).toHaveLength(1);
    });
    expect(server.puts).toHaveLength(0);
  });

  it('inserts a placeholder where the cursor is', async () => {
    promptServer();
    const section = await open();
    const text = area(section, 'Work template');
    await userEvent.click(text);
    text.setSelectionRange(5, 5);
    await userEvent.click(within(section).getByRole('button', { name: 'Insert agent' }));
    expect(text.value).toBe('Work {{agent}}{{ticketId}} as {{agent}}');
    expect(within(section).getByRole('status')).toHaveTextContent('Unsaved changes.');
  });

  it('previews the text on the screen for a key and says what is a stand-in', async () => {
    const server = promptServer();
    server.answerPreview({
      kind: 'work',
      key: 'ARC-0001',
      sample: false,
      prompt: 'You are worker-ARC-0001',
      standIns: ['feedback'],
    });
    const section = await open();
    await userEvent.type(area(section, 'Work template'), '!');
    await userEvent.type(within(section).getByRole('textbox', { name: 'Ticket key' }), 'arc-1');
    await userEvent.click(within(section).getByRole('button', { name: 'Preview' }));
    expect(await within(section).findByLabelText('Rendered instruction')).toHaveTextContent(
      'You are worker-ARC-0001',
    );
    expect(within(section).getByText('Stand-in text for feedback', { exact: false })).toBeVisible();
    expect(server.previews).toEqual([
      { kind: 'work', key: 'arc-1', template: 'Work {{ticketId}} as {{agent}}!' },
    ]);
  });

  it('previews with sample values for an empty key and asks for a question key in research', async () => {
    const server = promptServer();
    const section = await open();
    await userEvent.click(within(section).getByRole('button', { name: 'Research' }));
    expect(within(section).getByRole('textbox', { name: 'Question key' })).toBeInTheDocument();
    await userEvent.click(within(section).getByRole('button', { name: 'Preview' }));
    expect(await within(section).findByText('RENDERED')).toBeInTheDocument();
    expect(within(section).getByText('These are sample values', { exact: false })).toBeVisible();
    expect(server.previews).toEqual([
      { kind: 'research', key: null, template: 'Research {{questionId}}' },
    ]);
  });

  it('shows what a preview of a refused draft lacks', async () => {
    const server = promptServer();
    const section = await open();
    server.refuseNext(refused(['lacks required parts: Placeholder {{agent}}']));
    await userEvent.click(within(section).getByRole('button', { name: 'Preview' }));
    const list = await within(section).findByRole('list', { name: 'Preview problems' });
    expect(list).toHaveTextContent('lacks required parts: Placeholder {{agent}}');
  });

  it('says so when the configuration changed since the text was loaded', async () => {
    const server = promptServer();
    const section = await open();
    await userEvent.click(within(section).getByRole('button', { name: 'Planning' }));
    await userEvent.type(area(section, 'Planning template'), ' extra');
    server.bump();
    await userEvent.click(within(section).getByRole('button', { name: 'Save template' }));
    expect(
      await within(section).findByText(PROMPT_CONFLICT_MESSAGE, { exact: false }),
    ).toBeVisible();
    expect(area(section, 'Planning template').value).toContain('extra');
    await userEvent.click(within(section).getByRole('button', { name: 'Reload' }));
    expect(area(section, 'Planning template').value).toBe(DEFAULTS.plan);
    expect(within(section).queryByRole('button', { name: 'Reload' })).not.toBeInTheDocument();
  });

  it('says so when the prompts cannot be loaded', async () => {
    fakeApi({
      'GET /api/v1/dispatcher/config': () => json(200, configView()),
      'GET /api/v1/accounts': () => json(200, { items: [], accountsDir: '/data/accounts' }),
      'GET /api/v1/dispatcher/prompts': () => problem(500, 'internal', 'boom'),
    });
    renderApp('/settings');
    expect(await screen.findByText('Could not load the prompts.')).toBeInTheDocument();
  });
});
