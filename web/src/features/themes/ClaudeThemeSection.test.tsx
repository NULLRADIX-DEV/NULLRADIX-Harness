import { screen, waitFor } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import type { ClaudeTheme } from '../../shared/query/themes';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { claudeThemeHint } from './ClaudeThemeSection';

const ROUTE = '/api/v1/settings/claude-theme';

function status(overrides: Partial<ClaudeTheme> = {}): ClaudeTheme {
  return {
    enabled: false,
    state: 'none',
    themeId: 'night-blue',
    themeName: 'Night Blue',
    targets: [{ path: '/home/ada/.claude', state: 'none', message: null }],
    ...overrides,
  };
}

function server(initial: ClaudeTheme = status()) {
  let current = initial;
  return fakeApi({
    [`GET ${ROUTE}`]: () => json(200, current),
    [`PUT ${ROUTE}`]: async (request) => {
      const { enabled } = (await request.json()) as { enabled: boolean };
      current = status({
        enabled,
        state: enabled ? 'active' : 'none',
        targets: [{ path: '/home/ada/.claude', state: enabled ? 'active' : 'none', message: null }],
      });
      return json(200, current);
    },
  });
}

describe('ClaudeThemeSection', () => {
  it('shows the switch off until it is turned on', async () => {
    server();
    renderApp('/settings');
    const toggle = await screen.findByRole('switch', { name: 'Let Claude use the chosen theme' });
    await waitFor(() => {
      expect(toggle).toBeEnabled();
    });
    expect(toggle).not.toBeChecked();
    expect(screen.getByText('Off: Claude keeps its own look.')).toBeInTheDocument();
  });

  it('turns it on, sends the choice and shows the folders', async () => {
    const api = server();
    renderApp('/settings');
    const toggle = await screen.findByRole('switch', { name: 'Let Claude use the chosen theme' });
    await waitFor(() => {
      expect(toggle).toBeEnabled();
    });
    await userEvent.click(toggle);
    await waitFor(() => {
      expect(toggle).toBeChecked();
    });
    expect(await api.requests(`PUT ${ROUTE}`)[0]?.clone().json()).toEqual({ enabled: true });
    expect(await screen.findByText('Active in Claude: Night Blue')).toBeInTheDocument();
    expect(screen.getByRole('list', { name: 'Claude folders' })).toHaveTextContent(
      '/home/ada/.claude',
    );
  });

  it('shows why a folder failed', async () => {
    server(
      status({
        enabled: true,
        state: 'foreign',
        targets: [{ path: '/c/.claude', state: 'foreign', message: 'not owned by NULLRADIX' }],
      }),
    );
    renderApp('/settings');
    expect(await screen.findByText('not owned by NULLRADIX')).toBeInTheDocument();
    expect(
      screen.getByText('Night Blue could not be put into every Claude folder.'),
    ).toBeInTheDocument();
  });

  it('shows a refused change', async () => {
    const api = server();
    api.set(`PUT ${ROUTE}`, () => problem(422, 'bad-request', 'nope'));
    renderApp('/settings');
    const toggle = await screen.findByRole('switch', { name: 'Let Claude use the chosen theme' });
    await waitFor(() => {
      expect(toggle).toBeEnabled();
    });
    await userEvent.click(toggle);
    expect(await screen.findByText('nope')).toBeInTheDocument();
  });

  it('words every state', () => {
    expect(claudeThemeHint(status({ enabled: true, state: 'none' }))).toBe(
      'The chosen theme brings no Claude theme.',
    );
    expect(claudeThemeHint(status({ enabled: true, state: 'error', themeName: null }))).toBe(
      'night-blue could not be put into every Claude folder.',
    );
  });
});
