import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { configView, defaultConfig } from '../../test/dispatcherFixture';
import { FakeEventSource } from '../../test/eventSource';
import { fakeApi, json, problem, type Handler } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { CONFLICT_MESSAGE, INVALID_MESSAGE } from './DispatcherSection';
import type { DispatcherConfig, DispatcherConfigView } from './queries';

interface Saved {
  version: number;
  config: DispatcherConfig;
}

const account = (slug: string, name: string, enabled = true) => ({
  id: `0190a0b0-0000-7000-8000-00000000000${slug.length.toString()}`,
  slug,
  name,
  configDir: null,
  enabled,
  fallback: false,
  limitFiveHour: null,
  limitWeek: null,
  hasRunToken: false,
  createdAt: 1,
  updatedAt: 1,
  version: 1,
});

// In-memory daemon for the dispatcher routes; `refuse` answers the next save with a problem.
function dispatcherServer(
  initial: DispatcherConfigView = configView(),
  options: { accounts?: ReturnType<typeof account>[] } = {},
) {
  let view = initial;
  let refuse: Handler | null = null;
  const saves: Saved[] = [];
  const api = fakeApi({
    'GET /api/v1/dispatcher/config': () => json(200, view),
    'PUT /api/v1/dispatcher/config': async (request) => {
      const body = (await request.json()) as Saved;
      saves.push(body);
      if (refuse) {
        const answer = refuse;
        refuse = null;
        return answer(request);
      }
      if (body.version !== view.version) return problem(409, 'version-conflict', 'stale');
      view = { ...view, config: body.config, version: view.version + 1 };
      return json(200, view);
    },
    'GET /api/v1/accounts': () =>
      json(200, {
        items: options.accounts ?? [account('default', 'Default')],
        accountsDir: '/data/accounts',
      }),
  });
  return {
    api,
    saves,
    refuseNext(handler: Handler) {
      refuse = handler;
    },
    change(config: DispatcherConfig) {
      view = { ...view, config, version: view.version + 1 };
    },
    get view() {
      return view;
    },
  };
}

// Problem details with the refused fields, as the daemon sends them.
function invalid(body: Record<string, unknown>): Response {
  return new Response(JSON.stringify(body), {
    status: 422,
    headers: { 'content-type': 'application/problem+json' },
  });
}

async function open() {
  renderApp('/settings');
  return screen.findByRole('form', { name: 'Dispatcher configuration' });
}

function textbox(form: HTMLElement, name: string): HTMLInputElement {
  return within(form).getByRole<HTMLInputElement>('textbox', { name });
}

async function save(form: HTMLElement) {
  await userEvent.click(within(form).getByRole('button', { name: 'Save dispatcher settings' }));
}

describe('DispatcherSection', () => {
  it('shows the configuration in force, grouped', async () => {
    dispatcherServer();
    const form = await open();
    for (const name of [
      'Operation',
      'Autopilot',
      'Merging',
      'Profiles',
      'Escalation ladder',
      'Rules',
      'Accounts',
      'Research',
      'Planning',
      'Claude',
    ]) {
      expect(within(form).getByRole('heading', { name })).toBeInTheDocument();
    }
    expect(within(form).getByLabelText('Dispatcher on')).not.toBeChecked();
    expect(textbox(form, 'Parallel tickets (WIP)').value).toBe('1');
    expect(textbox(form, 'Never tickets with label').value).toBe('manual');
    expect(textbox(form, 'Profile 2 name').value).toBe('medium');
    expect(textbox(form, 'Profile 3 turns').value).toBe('150');
    expect(within(form).getByLabelText('Default profile')).toHaveValue('medium');
    expect(within(form).getByLabelText('Ladder step 3 profile')).toHaveValue('large');
    expect(textbox(form, 'Rule 4 files').value).toBe('6');
    expect(within(form).getByLabelText('Rule 2 profile')).toHaveValue('large');
    expect(within(form).getByLabelText('Merge strategy')).toHaveValue('merge');
    expect(within(form).getByLabelText('Account choice')).toHaveValue('most-free');
    expect(within(form).getByRole('button', { name: 'Save dispatcher settings' })).toBeDisabled();
  });

  it('saves an edit on the version it was based on and shows the result', async () => {
    const server = dispatcherServer();
    const form = await open();
    await userEvent.click(within(form).getByLabelText('Dispatcher on'));
    const wip = textbox(form, 'Parallel tickets (WIP)');
    await userEvent.clear(wip);
    await userEvent.type(wip, '3');
    await userEvent.type(textbox(form, 'Window from (HH:MM)'), '18:00');
    await userEvent.type(textbox(form, 'Window to (HH:MM)'), '07:00');
    await userEvent.selectOptions(within(form).getByLabelText('Merge strategy'), 'squash');
    expect(within(form).getByRole('status')).toHaveTextContent('Unsaved changes.');
    await save(form);

    await waitFor(() => {
      expect(within(form).getByRole('status')).toHaveTextContent('Saved.');
    });
    expect(server.saves).toHaveLength(1);
    const sent = server.saves[0];
    expect(sent?.version).toBe(1);
    expect(sent?.config).toMatchObject({
      enabled: true,
      wip: 3,
      window: { from: '18:00', to: '07:00' },
      mergeStrategy: 'squash',
      profiles: defaultConfig().profiles,
    });
    expect(server.view.version).toBe(2);
    expect(within(form).getByRole('button', { name: 'Save dispatcher settings' })).toBeDisabled();
  });

  it('keeps wrong numbers from being sent and marks them', async () => {
    const server = dispatcherServer();
    const form = await open();
    const wip = textbox(form, 'Parallel tickets (WIP)');
    await userEvent.clear(wip);
    await userEvent.type(wip, 'many');
    await save(form);
    expect(await within(form).findByText('Enter a whole number from 0.')).toBeInTheDocument();
    expect(wip).toHaveAttribute('aria-invalid', 'true');
    expect(server.saves).toHaveLength(0);

    await userEvent.clear(wip);
    await userEvent.type(wip, '2');
    expect(within(form).queryByText('Enter a whole number from 0.')).not.toBeInTheDocument();
    await save(form);
    await waitFor(() => {
      expect(server.saves).toHaveLength(1);
    });
  });

  it('wants both window times', async () => {
    const server = dispatcherServer();
    const form = await open();
    await userEvent.type(textbox(form, 'Window from (HH:MM)'), '08:00');
    await save(form);
    expect(
      await within(form).findByText('Fill in both times, or leave both empty.'),
    ).toBeInTheDocument();
    expect(server.saves).toHaveLength(0);
  });

  it('shows the fields the daemon refuses and keeps the edits', async () => {
    const server = dispatcherServer();
    const form = await open();
    const turns = textbox(form, 'Profile 2 turns');
    await userEvent.clear(turns);
    await userEvent.type(turns, '5');
    server.refuseNext(() =>
      invalid({
        type: 'about:blank',
        title: 'Unprocessable Entity',
        status: 422,
        code: 'dispatcher-config-invalid',
        detail: 'the dispatcher configuration has invalid fields',
        errors: [
          { field: 'profiles[1].maxTurns', message: 'must be a whole number from 10' },
          { field: 'accountLimitPercent', message: 'must be a whole number from 50 to 100' },
        ],
      }),
    );
    await save(form);

    expect(await within(form).findByText(INVALID_MESSAGE)).toBeInTheDocument();
    expect(within(form).getByText('maxTurns: must be a whole number from 10')).toBeInTheDocument();
    expect(within(form).getByText('must be a whole number from 50 to 100')).toBeInTheDocument();
    expect(turns.value).toBe('5');
    await userEvent.type(turns, '0');
    expect(within(form).queryByText(INVALID_MESSAGE)).not.toBeInTheDocument();
  });

  it('marks the fields that need a confirmation and tells how to get it', async () => {
    const server = dispatcherServer();
    const form = await open();
    const command = textbox(form, 'Claude command');
    await userEvent.clear(command);
    await userEvent.type(command, '/opt/claude');
    server.refuseNext(
      () =>
        new Response(
          JSON.stringify({
            type: 'about:blank',
            title: 'Forbidden',
            status: 403,
            code: 'step-up-required',
            detail: 'needs a confirmation',
            errors: [
              { field: 'claude.command', message: 'confirm with `nullradix confirm`' },
              { field: 'claude.allowedTools', message: 'confirm with `nullradix confirm`' },
              { field: 'worktreeSetup', message: 'confirm with `nullradix confirm`' },
            ],
          }),
          { status: 403, headers: { 'content-type': 'application/problem+json' } },
        ),
    );
    await save(form);

    const alerts = await within(form).findAllByText(/nullradix confirm/);
    expect(alerts.length).toBeGreaterThanOrEqual(4);
    expect(command).toHaveAttribute('aria-invalid', 'true');
    expect(command.parentElement).toHaveTextContent('confirm with `nullradix confirm`');
    const tools = within(form).getByRole('textbox', { name: /Allowed tools/ });
    expect(tools.parentElement).toHaveTextContent('confirm with `nullradix confirm`');
    const steps = within(form).getByRole('list', { name: 'Worktree setup steps' });
    expect(steps.parentElement).toHaveTextContent('confirm with `nullradix confirm`');
    expect(
      within(form).getByText(/The fields that run commands are marked below\./),
    ).toBeInTheDocument();
    expect(command).toHaveValue('/opt/claude');
  });

  it('shows that the session is not confirmed and asks for a code to turn the check off', async () => {
    const server = dispatcherServer();
    const form = await open();
    const toggle = await within(form).findByRole('checkbox', {
      name: 'Ask for a confirmation before changing commands',
    });
    expect(toggle).toBeChecked();
    expect(within(form).getByText(/Not confirmed\./)).toBeInTheDocument();
    await userEvent.click(toggle);
    expect(
      within(form).getByText(/nullradix confirm --turn-off .* A confirmed session is not enough/),
    ).toBeInTheDocument();
    // nothing is sent before a code is typed, and the switch stays on until the daemon agrees
    expect(server.api.requests('PUT /api/v1/settings/step-up')).toHaveLength(0);
    expect(toggle).toBeChecked();
    expect(within(form).getByRole('button', { name: 'Turn off' })).toBeDisabled();
  });

  it('turns the check off with a fresh code even in a confirmed session', async () => {
    const server = dispatcherServer();
    let required = true;
    server.api.set('GET /api/v1/auth/me', () =>
      json(200, { kind: 'session', elevatedUntil: Date.now() + 600_000 }),
    );
    server.api.set('GET /api/v1/settings/step-up', () => json(200, { required }));
    server.api.set('PUT /api/v1/settings/step-up', () => {
      required = false;
      return json(200, { required });
    });
    const form = await open();
    const toggle = await within(form).findByRole('checkbox', {
      name: 'Ask for a confirmation before changing commands',
    });
    await userEvent.click(toggle);
    await userEvent.type(
      within(form).getByRole('textbox', { name: 'Confirmation code' }),
      ' abc123 ',
    );
    await userEvent.click(within(form).getByRole('button', { name: 'Turn off' }));
    await waitFor(() => {
      expect(toggle).not.toBeChecked();
    });
    const [request] = server.api.requests('PUT /api/v1/settings/step-up');
    expect(await request?.json()).toEqual({ required: false, code: 'abc123' });
  });

  it('keeps the check on and says so when the code for turning it off is refused', async () => {
    const server = dispatcherServer();
    server.api.set('PUT /api/v1/settings/step-up', () =>
      problem(401, 'step-up-code-invalid', 'expired'),
    );
    const form = await open();
    const toggle = await within(form).findByRole('checkbox', {
      name: 'Ask for a confirmation before changing commands',
    });
    await userEvent.click(toggle);
    await userEvent.type(within(form).getByRole('textbox', { name: 'Confirmation code' }), 'bad');
    await userEvent.click(within(form).getByRole('button', { name: 'Turn off' }));
    expect(await within(form).findByRole('alert')).toHaveTextContent(
      'The code is unknown, expired or no turn-off code. Run nullradix confirm --turn-off again.',
    );
    expect(toggle).toBeChecked();
    // cancelling goes back to the plain confirm form
    await userEvent.click(within(form).getByRole('button', { name: 'Cancel' }));
    expect(within(form).getByRole('button', { name: 'Confirm' })).toBeInTheDocument();
    expect(within(form).queryByRole('alert')).not.toBeInTheDocument();
  });

  it('turns the check on again without a code', async () => {
    const server = dispatcherServer();
    server.api.set('GET /api/v1/settings/step-up', () => json(200, { required: false }));
    server.api.set('PUT /api/v1/settings/step-up', () => json(200, { required: true }));
    const form = await open();
    const toggle = await within(form).findByRole('checkbox', {
      name: 'Ask for a confirmation before changing commands',
    });
    await waitFor(() => {
      expect(toggle).not.toBeChecked();
    });
    await userEvent.click(toggle);
    await waitFor(() => {
      expect(server.api.requests('PUT /api/v1/settings/step-up')).toHaveLength(1);
    });
    const [request] = server.api.requests('PUT /api/v1/settings/step-up');
    expect(await request?.json()).toEqual({ required: true });
  });

  it('sends a typed confirmation code and shows when the confirmation ends', async () => {
    const server = dispatcherServer();
    let elevated: number | null = null;
    server.api.set('GET /api/v1/auth/me', () =>
      json(200, { kind: 'session', elevatedUntil: elevated }),
    );
    server.api.set('POST /api/v1/auth/step-up', () => {
      elevated = Date.now() + 600_000;
      return json(200, { elevatedUntil: elevated });
    });
    const form = await open();
    await userEvent.type(
      await within(form).findByRole('textbox', { name: 'Confirmation code' }),
      'abc123',
    );
    await userEvent.click(within(form).getByRole('button', { name: 'Confirm' }));
    expect(await within(form).findByText(/Confirmed until/)).toBeInTheDocument();
    const [request] = server.api.requests('POST /api/v1/auth/step-up');
    expect(await request?.json()).toEqual({ code: 'abc123' });
  });

  it('confirms with Enter in the code box without saving the dispatcher form', async () => {
    const server = dispatcherServer();
    server.api.set('POST /api/v1/auth/step-up', () =>
      json(200, { elevatedUntil: Date.now() + 600_000 }),
    );
    const form = await open();
    const box = await within(form).findByRole('textbox', { name: 'Confirmation code' });
    await userEvent.type(box, 'abc123{Enter}');
    await waitFor(() => {
      expect(server.api.requests('POST /api/v1/auth/step-up')).toHaveLength(1);
    });
    expect(server.saves).toHaveLength(0);
  });

  it('follows a renamed profile through the default, the ladder and the rules', async () => {
    const server = dispatcherServer();
    const form = await open();
    const name = textbox(form, 'Profile 3 name');
    await userEvent.clear(name);
    await userEvent.type(name, 'huge');
    expect(within(form).getByLabelText('Rule 2 profile')).toHaveValue('huge');
    expect(within(form).getByLabelText('Ladder step 3 profile')).toHaveValue('huge');
    await save(form);

    await waitFor(() => {
      expect(server.saves).toHaveLength(1);
    });
    const config = server.saves[0]?.config;
    expect(config?.profiles.map((profile) => profile.name)).toEqual(['small', 'medium', 'huge']);
    expect(config?.ladder).toEqual(['small', 'medium', 'huge']);
    expect(config?.rules.map((rule) => rule.profile)).toEqual(['small', 'huge', 'huge', 'huge']);
  });

  it('empties the references of a removed profile and asks for new ones', async () => {
    const server = dispatcherServer();
    const form = await open();
    await userEvent.click(within(form).getByRole('button', { name: 'Remove profile 3' }));
    expect(within(form).queryByLabelText('Profile 3 name')).not.toBeInTheDocument();
    expect(within(form).queryByLabelText('Ladder step 3 profile')).not.toBeInTheDocument();
    expect(within(form).getByLabelText('Rule 2 profile')).toHaveValue('');

    await save(form);
    expect((await within(form).findAllByText(/Pick a profile./)).length).toBeGreaterThanOrEqual(3);
    expect(server.saves).toHaveLength(0);

    for (const rule of [2, 3, 4]) {
      await userEvent.selectOptions(
        within(form).getByLabelText(`Rule ${String(rule)} profile`),
        'medium',
      );
    }
    await save(form);
    await waitFor(() => {
      expect(server.saves).toHaveLength(1);
    });
    expect(server.saves[0]?.config.rules.map((rule) => rule.profile)).toEqual([
      'small',
      'medium',
      'medium',
      'medium',
    ]);
  });

  it('adds and removes profiles, ladder steps and rules and moves profiles', async () => {
    const server = dispatcherServer();
    const form = await open();
    await userEvent.click(within(form).getByRole('button', { name: 'Move profile 1 down' }));
    expect(textbox(form, 'Profile 1 name').value).toBe('medium');
    expect(within(form).getByRole('button', { name: 'Move profile 3 down' })).toBeDisabled();

    await userEvent.click(within(form).getByRole('button', { name: 'Add profile' }));
    await userEvent.type(textbox(form, 'Profile 4 name'), 'tiny');
    await userEvent.click(within(form).getByRole('button', { name: 'Add step' }));
    expect(within(form).getByLabelText('Ladder step 4 profile')).toHaveValue('tiny');
    await userEvent.click(within(form).getByRole('button', { name: 'Remove ladder step 1' }));

    await userEvent.click(within(form).getByRole('button', { name: 'Add rule' }));
    await userEvent.type(textbox(form, 'Rule 5 labels'), 'quick, easy');
    await userEvent.click(within(form).getByRole('button', { name: 'Move rule 5 up' }));
    await userEvent.click(within(form).getByRole('button', { name: 'Remove rule 1' }));
    await save(form);

    await waitFor(() => {
      expect(server.saves).toHaveLength(1);
    });
    const config = server.saves[0]?.config;
    expect(config?.profiles.map((profile) => profile.name)).toEqual([
      'medium',
      'small',
      'large',
      'tiny',
    ]);
    expect(config?.profiles[3]).toMatchObject({ maxTurns: 80, timeoutMinutes: 60 });
    expect(config?.ladder).toEqual(['medium', 'large', 'tiny']);
    expect(config?.rules).toHaveLength(4);
    expect(config?.rules[2]?.when.labels).toEqual(['quick', 'easy']);
  });

  it('asks for a condition and a profile on every rule', async () => {
    const server = dispatcherServer();
    const form = await open();
    await userEvent.click(within(form).getByRole('button', { name: 'Add rule' }));
    await save(form);
    expect(await within(form).findByText('when: Set at least one condition.')).toBeInTheDocument();
    expect(server.saves).toHaveLength(0);
  });

  it('picks accounts for profiles and rules in the order they were ticked', async () => {
    const server = dispatcherServer(configView(), {
      accounts: [
        account('default', 'Default'),
        account('max', 'Max'),
        account('work', 'Work', false),
      ],
    });
    const form = await open();
    const picker = await within(form).findByRole('group', { name: 'Profile 3 accounts' });
    expect(within(picker).getByText('any')).toBeInTheDocument();
    await userEvent.click(within(picker).getByText('any'));
    await userEvent.click(within(picker).getByLabelText('Profile 3 accounts: Work'));
    await userEvent.click(within(picker).getByLabelText('Profile 3 accounts: Max'));
    expect(within(picker).getByText('Work, Max')).toBeInTheDocument();
    expect(within(picker).getByText('off')).toBeInTheDocument();
    const rulePicker = within(form).getByRole('group', { name: 'Rule 1 accounts' });
    await userEvent.click(within(rulePicker).getByText('as profile'));
    await userEvent.click(within(rulePicker).getByLabelText('Rule 1 accounts: Max'));
    await save(form);

    await waitFor(() => {
      expect(server.saves).toHaveLength(1);
    });
    const config = server.saves[0]?.config;
    expect(config?.profiles[2]?.accounts).toEqual(['work', 'max']);
    expect(config?.rules[0]?.accounts).toEqual(['max']);
  });

  it('keeps an account that is gone visible so it can be unticked', async () => {
    const config = defaultConfig();
    if (config.profiles[0]) config.profiles[0].accounts = ['gone'];
    dispatcherServer(configView(config));
    const form = await open();
    const picker = await within(form).findByRole('group', { name: 'Profile 1 accounts' });
    expect(within(picker).getByText('unknown')).toBeInTheDocument();
    await userEvent.click(within(picker).getByLabelText('Profile 1 accounts: gone'));
    expect(within(picker).getByText('any')).toBeInTheDocument();
  });

  it('reports a stale version and loads the new one on request', async () => {
    const server = dispatcherServer();
    const form = await open();
    await userEvent.click(within(form).getByLabelText('Dispatcher on'));
    const other = defaultConfig();
    other.wip = 7;
    server.change(other);
    await save(form);

    expect(await within(form).findByText(CONFLICT_MESSAGE)).toBeInTheDocument();
    expect(textbox(form, 'Parallel tickets (WIP)').value).toBe('1');
    await userEvent.click(within(form).getByRole('button', { name: 'Reload' }));
    await waitFor(() => {
      expect(textbox(form, 'Parallel tickets (WIP)').value).toBe('7');
    });
    expect(within(form).getByLabelText('Dispatcher on')).not.toBeChecked();
    expect(within(form).queryByText(CONFLICT_MESSAGE)).not.toBeInTheDocument();
  });

  it('adopts a change from elsewhere while nothing is being edited', async () => {
    const server = dispatcherServer();
    const form = await open();
    await waitFor(() => {
      expect(FakeEventSource.instances).not.toHaveLength(0);
    });
    const other = defaultConfig();
    other.wip = 4;
    server.change(other);
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        { seq: 5, entity: 'dispatcher-config', entityId: 'x', op: 'update', at: 2 },
        '5',
      );
    });
    await waitFor(() => {
      expect(textbox(form, 'Parallel tickets (WIP)').value).toBe('4');
    });
  });

  it('keeps unsaved edits and says so when the configuration changed elsewhere', async () => {
    const server = dispatcherServer();
    const form = await open();
    await waitFor(() => {
      expect(FakeEventSource.instances).not.toHaveLength(0);
    });
    await userEvent.type(textbox(form, 'Window from (HH:MM)'), '09:00');
    const other = defaultConfig();
    other.wip = 4;
    server.change(other);
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        { seq: 5, entity: 'dispatcher-config', entityId: 'x', op: 'update', at: 2 },
        '5',
      );
    });
    expect(
      await within(form).findByText('The configuration was changed elsewhere.', { exact: false }),
    ).toBeInTheDocument();
    expect(textbox(form, 'Window from (HH:MM)').value).toBe('09:00');
    expect(textbox(form, 'Parallel tickets (WIP)').value).toBe('1');
  });

  it('edits the worktree setup: add, order, remove and save', async () => {
    const server = dispatcherServer(
      configView({
        ...defaultConfig(),
        worktreeSetup: [{ cwd: '.', run: 'npm ci' }],
      }),
    );
    const form = await open();
    expect(textbox(form, 'Setup step 1 command').value).toBe('npm ci');
    expect(textbox(form, 'Setup timeout (minutes)').value).toBe('15');
    await userEvent.click(within(form).getByRole('button', { name: 'Add setup step' }));
    await userEvent.type(textbox(form, 'Setup step 2 folder'), 'web');
    await userEvent.type(textbox(form, 'Setup step 2 command'), 'npm run build');
    await userEvent.click(within(form).getByRole('button', { name: 'Move setup step 2 up' }));
    expect(textbox(form, 'Setup step 1 command').value).toBe('npm run build');
    await userEvent.click(within(form).getByRole('button', { name: 'Remove setup step 2' }));
    await userEvent.click(within(form).getByRole('button', { name: 'Add setup step' }));
    await userEvent.type(textbox(form, 'Setup step 2 command'), 'npm test');
    await save(form);
    await waitFor(() => {
      expect(server.saves).toHaveLength(1);
    });
    expect(server.saves[0]?.config.worktreeSetup).toEqual([
      { cwd: 'web', run: 'npm run build' },
      { cwd: '.', run: 'npm test' },
    ]);
  });

  it('wants a command in every setup step and shows the folder the daemon refuses', async () => {
    const server = dispatcherServer();
    const form = await open();
    await userEvent.click(within(form).getByRole('button', { name: 'Add setup step' }));
    await save(form);
    expect(await within(form).findByText('run: Enter a command.')).toBeInTheDocument();
    expect(server.saves).toHaveLength(0);
    await userEvent.type(textbox(form, 'Setup step 1 command'), 'make');
    server.refuseNext(() =>
      invalid({
        type: 'about:blank',
        title: 'Unprocessable Entity',
        status: 422,
        code: 'dispatcher-config-invalid',
        detail: 'the dispatcher configuration has invalid fields',
        errors: [{ field: 'worktreeSetup[0].cwd', message: 'leaves the worktree' }],
      }),
    );
    await save(form);
    expect(await within(form).findByText('cwd: leaves the worktree')).toBeInTheDocument();
  });

  it('says so when the settings cannot be loaded', async () => {
    fakeApi({ 'GET /api/v1/dispatcher/config': () => problem(500, 'internal', 'boom') });
    renderApp('/settings');
    expect(await screen.findByText('Could not load the dispatcher settings.')).toBeInTheDocument();
  });
});
