import { screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { fakeApi, json } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { Favorite } from './favoriteQueries';
import type { Session } from './queries';

const ID = '0190a0b0-aaaa-7000-8000-000000000001';

function session(): Session {
  const now = Date.now();
  return {
    id: ID,
    label: 'SE-001: Fix the login',
    title: 'Fix the login',
    number: 1,
    cwd: '/work/arches',
    state: 'running',
    displayState: 'running',
    listening: true,
    startedAt: now - 60_000,
    lastEventAt: now,
    endedAt: null,
    runtimeMs: 60_000,
    client: 'claude-code',
    subagents: [],
    tickets: [],
    version: 2,
  };
}

function favorite(overrides: Partial<Favorite> = {}): Favorite {
  return {
    sessionId: ID,
    note: 'login fix',
    tickets: ['ARC-0001'],
    cwd: '/work/arches',
    cwdExists: false,
    title: 'Fix the login',
    label: 'SE-001: Fix the login',
    markedAt: Date.now(),
    state: 'ended',
    displayState: 'ended',
    live: false,
    lastEventAt: Date.now(),
    resumedAs: [{ sessionId: 'fedcba98-0000-7000-8000-000000000002', at: Date.now() }],
    command: "cd '/work/arches' && claude --resume " + ID,
    version: 1,
    ...overrides,
  };
}

function server(favorites: Favorite[]) {
  let items = favorites;
  const api = fakeApi({
    'GET /api/v1/sessions': () => json(200, { items: [session()] }),
    'GET /api/v1/favorites': () => json(200, { items }),
    [`PUT /api/v1/sessions/${ID}/favorite`]: () => {
      items = [favorite()];
      return json(200, items[0]);
    },
    [`DELETE /api/v1/sessions/${ID}/favorite`]: () => {
      items = [];
      return new Response(null, { status: 204 });
    },
  });
  return api;
}

describe('Favorites', () => {
  it('lists a favorite with its note, command and resumptions', async () => {
    server([favorite()]);
    renderApp('/sessions');
    const card = await screen.findByRole('article', { name: `Favorite ${ID.slice(0, 8)}` });
    expect(within(card).getByDisplayValue('login fix')).toBeInTheDocument();
    expect(
      within(card).getByText(`cd '/work/arches' && claude --resume ${ID}`),
    ).toBeInTheDocument();
    expect(within(card).getByText('Resumed as fedcba98')).toBeInTheDocument();
    expect(within(card).getByRole('alert')).toHaveTextContent('The folder is gone.');
    expect(within(card).getByText('(no longer in the session list)')).toBeInTheDocument();
    expect(within(card).getByRole('link', { name: 'ARC-0001' })).toHaveAttribute(
      'href',
      '/board?ticket=ARC-0001',
    );
  });

  it('marks with the star, saves a note and removes the favorite', async () => {
    const api = server([]);
    renderApp('/sessions');
    const star = await screen.findByRole('button', { name: 'Add to favorites' });
    await userEvent.click(star);
    expect(await screen.findByRole('button', { name: 'Remove from favorites' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    const card = await screen.findByRole('article', { name: `Favorite ${ID.slice(0, 8)}` });
    const note = within(card).getByRole('textbox');
    await userEvent.clear(note);
    await userEvent.type(note, 'keep this');
    await userEvent.click(within(card).getByRole('button', { name: 'Save note' }));
    await waitFor(() => {
      expect(api.requests(`PUT /api/v1/sessions/${ID}/favorite`)).toHaveLength(2);
    });
    const [, saved] = api.requests(`PUT /api/v1/sessions/${ID}/favorite`);
    expect(await saved?.json()).toEqual({ note: 'keep this' });
    await userEvent.click(within(card).getByRole('button', { name: 'Remove favorite' }));
    await waitFor(() => {
      expect(screen.queryByRole('region', { name: 'Favorites' })).not.toBeInTheDocument();
    });
    expect(api.requests(`DELETE /api/v1/sessions/${ID}/favorite`)).toHaveLength(1);
  });
});
