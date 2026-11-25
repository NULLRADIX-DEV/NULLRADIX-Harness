import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { FakeEventSource } from '../../test/eventSource';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { CONFLICT_MESSAGE, LIMIT_MESSAGE } from './AccountsSection';
import type { Account } from './queries';

const DEFAULT_ID = '0190a0b0-0000-7000-8000-000000000001';
const MAX_ID = '0190a0b0-0000-7000-8000-000000000002';
const ACCOUNTS_DIR = '/data/accounts';
const FAKE_TOKEN = 'sk-ant-oat01-fake-token-for-tests';

function account(overrides: Partial<Account> = {}): Account {
  return {
    id: MAX_ID,
    slug: 'max',
    name: 'Max',
    configDir: `${ACCOUNTS_DIR}/max`,
    enabled: true,
    fallback: false,
    limitFiveHour: null,
    limitWeek: null,
    hasRunToken: false,
    createdAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

function defaultAccount(): Account {
  return account({ id: DEFAULT_ID, slug: 'default', name: 'Default', configDir: null });
}

// In-memory daemon for the accounts routes.
function accountsServer(initial: Account[] = [defaultAccount(), account()]) {
  let items = [...initial];
  const find = (request: Request) => {
    const id = new URL(request.url).pathname.split('/')[4];
    return items.find((item) => item.id === id);
  };
  const change = (id: string, patch: Partial<Account>) => {
    items = items.map((item) =>
      item.id === id ? { ...item, ...patch, version: item.version + 1 } : item,
    );
    return items.find((item) => item.id === id);
  };
  const api = fakeApi({
    'GET /api/v1/accounts': () => json(200, { items, accountsDir: ACCOUNTS_DIR }),
    'POST /api/v1/accounts': async (request) => {
      const body = (await request.json()) as { name: string; configDir?: string };
      const created = account({
        id: `0190a0b0-0000-7000-8000-00000000001${String(items.length)}`,
        slug: body.name.toLowerCase(),
        name: body.name,
        configDir: body.configDir ?? `${ACCOUNTS_DIR}/${body.name.toLowerCase()}`,
      });
      items = [...items, created];
      return json(201, created);
    },
    [`PATCH /api/v1/accounts/${MAX_ID}`]: async (request) => {
      const { version, ...patch } = (await request.json()) as Partial<Account> & {
        version: number;
      };
      const current = find(request);
      if (!current) return problem(404, 'account-not-found', 'no such account');
      if (current.version !== version) return problem(409, 'version-conflict', 'stale');
      return json(200, change(current.id, patch));
    },
    [`PUT /api/v1/accounts/${MAX_ID}/run-token`]: async (request) => {
      const body = (await request.json()) as { token: string; version: number };
      const current = find(request);
      if (!current) return problem(404, 'account-not-found', 'no such account');
      if (!body.token.startsWith('sk-ant-oat')) {
        return problem(422, 'run-token-invalid', 'a run token starts with sk-ant-oat');
      }
      return json(200, change(current.id, { hasRunToken: true }));
    },
    [`DELETE /api/v1/accounts/${MAX_ID}/run-token`]: (request) => {
      const current = find(request);
      if (!current) return problem(404, 'account-not-found', 'no such account');
      return json(200, change(current.id, { hasRunToken: false }));
    },
    [`DELETE /api/v1/accounts/${MAX_ID}`]: (request) => {
      const version = Number(new URL(request.url).searchParams.get('version'));
      const current = find(request);
      if (!current) return problem(404, 'account-not-found', 'no such account');
      if (current.version !== version) return problem(409, 'version-conflict', 'stale');
      items = items.filter((item) => item.id !== current.id);
      return new Response(null, { status: 204 });
    },
  });
  return {
    api,
    // Simulates a change made by another client.
    rename(name: string) {
      change(MAX_ID, { name });
    },
  };
}

async function row(name: string) {
  return within(await screen.findByRole('listitem', { name }));
}

describe('AccountsSection', () => {
  it('lists the accounts with their switches, limits and folders', async () => {
    accountsServer([
      defaultAccount(),
      account({ fallback: true, limitFiveHour: 50, limitWeek: 80, hasRunToken: true }),
    ]);
    renderApp('/settings');
    const max = await row('Max');
    expect(max.getByText('max')).toBeInTheDocument();
    expect(max.getByText('fallback')).toBeInTheDocument();
    expect(max.getByText(`${ACCOUNTS_DIR}/max`)).toBeInTheDocument();
    expect(max.getByRole('checkbox', { name: 'Enabled' })).toBeChecked();
    expect(max.getByRole('checkbox', { name: 'Only as fallback' })).toBeChecked();
    expect(max.getByRole('textbox', { name: '5 hours limit in percent' })).toHaveValue('50');
    expect(max.getByRole('textbox', { name: 'Week limit in percent' })).toHaveValue('80');
    expect(max.getByText('set')).toBeInTheDocument();
    const server = await row('Default');
    expect(server.getByText('server login')).toBeInTheDocument();
    expect(server.queryByRole('button', { name: 'Remove Default' })).not.toBeInTheDocument();
    expect(server.getByRole('button', { name: 'Rename Default' })).toBeInTheDocument();
  });

  it('adds an account with its own folder or an existing one', async () => {
    const { api } = accountsServer([defaultAccount()]);
    renderApp('/settings');
    await row('Default');
    const form = within(screen.getByRole('form', { name: 'New account' }));
    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'Work');
    await userEvent.click(form.getByRole('button', { name: 'Add account' }));
    expect(await row('Work')).toBeTruthy();
    const [first] = api.requests('POST /api/v1/accounts');
    expect(await first?.clone().json()).toEqual({ name: 'Work' });
    expect(form.getByRole('textbox', { name: 'Name' })).toHaveValue('');

    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'Max');
    await userEvent.click(screen.getByText('Use an existing Claude folder'));
    await userEvent.type(form.getByRole('textbox', { name: 'Folder' }), ' ~/.mcacc/max ');
    await userEvent.click(form.getByRole('button', { name: 'Add account' }));
    await waitFor(() => {
      expect(api.requests('POST /api/v1/accounts')).toHaveLength(2);
    });
    const second = api.requests('POST /api/v1/accounts')[1];
    expect(await second?.clone().json()).toEqual({ name: 'Max', configDir: '~/.mcacc/max' });
  });

  it('shows what the daemon says about a refused folder', async () => {
    const { api } = accountsServer([defaultAccount()]);
    api.set('POST /api/v1/accounts', () =>
      problem(422, 'account-invalid', 'the folder gone does not exist'),
    );
    renderApp('/settings');
    await row('Default');
    const form = within(screen.getByRole('form', { name: 'New account' }));
    await userEvent.type(form.getByRole('textbox', { name: 'Name' }), 'Odd');
    await userEvent.click(form.getByRole('button', { name: 'Add account' }));
    expect(await form.findByRole('alert')).toHaveTextContent('the folder gone does not exist');
  });

  it('switches an account off and on with its version', async () => {
    const { api } = accountsServer();
    renderApp('/settings');
    const max = await row('Max');
    await userEvent.click(max.getByRole('checkbox', { name: 'Enabled' }));
    await waitFor(() => {
      expect(max.getByRole('checkbox', { name: 'Enabled' })).not.toBeChecked();
    });
    expect(max.getByText('disabled')).toBeInTheDocument();
    expect(max.getByRole('checkbox', { name: 'Only as fallback' })).toBeDisabled();
    await userEvent.click(max.getByRole('checkbox', { name: 'Enabled' }));
    await waitFor(() => {
      expect(max.getByRole('checkbox', { name: 'Enabled' })).toBeChecked();
    });
    const bodies = await Promise.all(
      api.requests(`PATCH /api/v1/accounts/${MAX_ID}`).map((request) => request.clone().json()),
    );
    expect(bodies).toEqual([
      { enabled: false, version: 1 },
      { enabled: true, version: 2 },
    ]);
  });

  it('saves a limit when the field is left and removes it when emptied', async () => {
    const { api } = accountsServer();
    renderApp('/settings');
    const max = await row('Max');
    const week = max.getByRole('textbox', { name: 'Week limit in percent' });
    await userEvent.type(week, '80');
    await userEvent.tab();
    await waitFor(() => {
      expect(api.requests(`PATCH /api/v1/accounts/${MAX_ID}`)).toHaveLength(1);
    });
    await waitFor(() => {
      expect(week).toBeEnabled();
    });
    await userEvent.clear(week);
    await userEvent.type(week, '{Enter}');
    await waitFor(() => {
      expect(api.requests(`PATCH /api/v1/accounts/${MAX_ID}`)).toHaveLength(2);
    });
    const bodies = await Promise.all(
      api.requests(`PATCH /api/v1/accounts/${MAX_ID}`).map((request) => request.clone().json()),
    );
    expect(bodies).toEqual([
      { limitWeek: 80, version: 1 },
      { limitWeek: null, version: 2 },
    ]);
  });

  it('refuses a limit outside 1 to 100 without asking the daemon', async () => {
    const { api } = accountsServer();
    renderApp('/settings');
    const max = await row('Max');
    const five = max.getByRole('textbox', { name: '5 hours limit in percent' });
    for (const bad of ['0', '101', '5.5', 'many']) {
      await userEvent.clear(five);
      await userEvent.type(five, bad);
      await userEvent.tab();
      expect(await max.findByText(LIMIT_MESSAGE)).toBeInTheDocument();
    }
    expect(api.requests(`PATCH /api/v1/accounts/${MAX_ID}`)).toHaveLength(0);
  });

  it('sends the run token once, empties the field and never shows it again', async () => {
    const { api } = accountsServer();
    renderApp('/settings');
    const max = await row('Max');
    const input = max.getByLabelText('Run token');
    expect(input).toHaveAttribute('type', 'password');
    await userEvent.type(input, FAKE_TOKEN);
    await userEvent.click(max.getByRole('button', { name: 'Save token' }));

    expect(await max.findByText('set')).toBeInTheDocument();
    const [request] = api.requests(`PUT /api/v1/accounts/${MAX_ID}/run-token`);
    expect(await request?.clone().json()).toEqual({ token: FAKE_TOKEN, version: 1 });
    expect(max.queryByLabelText('Run token')).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain(FAKE_TOKEN);
    expect(screen.queryByDisplayValue(FAKE_TOKEN)).not.toBeInTheDocument();
  });

  it('explains a run token of the wrong shape and keeps no copy of it in the page', async () => {
    accountsServer();
    renderApp('/settings');
    const max = await row('Max');
    await userEvent.type(max.getByLabelText('Run token'), 'sk-ant-api03-not-a-run-token');
    await userEvent.click(max.getByRole('button', { name: 'Save token' }));
    expect(await max.findByRole('alert')).toHaveTextContent(
      'This does not look like a token of `claude setup-token`',
    );
    expect(document.body.textContent).not.toContain('sk-ant-api03-not-a-run-token');
  });

  it('removes a run token', async () => {
    const { api } = accountsServer([defaultAccount(), account({ hasRunToken: true })]);
    renderApp('/settings');
    const max = await row('Max');
    await userEvent.click(max.getByRole('button', { name: 'Remove the run token of Max' }));
    expect(await max.findByLabelText('Run token')).toBeInTheDocument();
    const [request] = api.requests(`DELETE /api/v1/accounts/${MAX_ID}/run-token`);
    expect(new URL(request?.url ?? '').search).toBe('?version=1');
  });

  it('renames an account and reports a conflict with a refetch', async () => {
    const server = accountsServer();
    renderApp('/settings');
    const max = await row('Max');
    await userEvent.click(max.getByRole('button', { name: 'Rename Max' }));
    server.rename('Max elsewhere');
    const form = within(screen.getByRole('form', { name: 'Rename Max' }));
    const input = form.getByRole('textbox', { name: 'Name' });
    await userEvent.clear(input);
    await userEvent.type(input, 'Mine');
    await userEvent.click(form.getByRole('button', { name: 'Save' }));
    expect(await form.findByRole('alert')).toHaveTextContent(CONFLICT_MESSAGE);
    expect(await screen.findByText('Max elsewhere')).toBeInTheDocument();
  });

  it('removes an account after confirmation and offers the folder only below the accounts folder', async () => {
    const { api } = accountsServer();
    renderApp('/settings');
    const max = await row('Max');
    await userEvent.click(max.getByRole('button', { name: 'Remove Max' }));
    const confirm = within(screen.getByRole('group', { name: 'Remove Max' }));
    await userEvent.click(confirm.getByRole('button', { name: 'Cancel' }));
    expect(api.requests(`DELETE /api/v1/accounts/${MAX_ID}`)).toHaveLength(0);

    await userEvent.click(max.getByRole('button', { name: 'Remove Max' }));
    const again = within(screen.getByRole('group', { name: 'Remove Max' }));
    await userEvent.click(again.getByRole('checkbox'));
    await userEvent.click(again.getByRole('button', { name: 'Remove' }));
    await waitFor(() => {
      expect(screen.queryByRole('listitem', { name: 'Max' })).not.toBeInTheDocument();
    });
    const [request] = api.requests(`DELETE /api/v1/accounts/${MAX_ID}`);
    expect(new URL(request?.url ?? '').search).toBe('?version=1&deleteDir=true');
  });

  it('does not offer to delete a folder the account does not own', async () => {
    accountsServer([defaultAccount(), account({ configDir: '/profiles/work' })]);
    renderApp('/settings');
    const max = await row('Max');
    await userEvent.click(max.getByRole('button', { name: 'Remove Max' }));
    const confirm = within(screen.getByRole('group', { name: 'Remove Max' }));
    expect(confirm.queryByRole('checkbox')).not.toBeInTheDocument();
  });

  it('refetches the list when the change stream reports an account change', async () => {
    const server = accountsServer();
    renderApp('/settings');
    await row('Max');
    await waitFor(() => {
      expect(FakeEventSource.instances).not.toHaveLength(0);
    });
    server.rename('Max live');
    act(() => {
      FakeEventSource.latest().emit(
        'change',
        { seq: 3, entity: 'account', entityId: MAX_ID, op: 'update', at: 2 },
        '3',
      );
    });
    expect(await screen.findByRole('listitem', { name: 'Max live' })).toBeInTheDocument();
  });
});
