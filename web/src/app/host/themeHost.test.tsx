import {
  useBoards,
  useColorScheme,
  useNav,
  useNavigate,
  useSettings,
  useThemeSettings,
  useUsage,
  type ShellProps,
} from '@nullradix/theme-sdk';
import { act, screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { BOARD_STORAGE_KEY } from '../../features/board/boardChoice';
import { boardKeys } from '../../features/boards/queries';
import { json } from '../../test/fakeApi';
import { BOARD_ID } from '../../test/boardServer';
import type { ThemeSettingSpec } from '../../shared/query/themes';
import { OPS_BOARD_ID, SIDE_BOARD_ID, renderShell } from '../../test/renderShell';

interface Seen {
  nav: ReturnType<typeof useNav>;
  boards: ReturnType<typeof useBoards>;
  usage: ReturnType<typeof useUsage>;
  settings: ReturnType<typeof useSettings>;
  themeSettings: ReturnType<typeof useThemeSettings>;
  scheme: ReturnType<typeof useColorScheme>;
  navigate: ReturnType<typeof useNavigate>;
}

const seen = vi.fn<(host: Seen) => void>();

// A shell that reads everything the host offers and reports it after each render.
function Probe({ content }: ShellProps) {
  const host: Seen = {
    nav: useNav(),
    boards: useBoards(),
    usage: useUsage(),
    settings: useSettings(),
    themeSettings: useThemeSettings(),
    scheme: useColorScheme(),
    navigate: useNavigate(),
  };
  useEffect(() => {
    seen(host);
  });
  return <>{content}</>;
}

function last(): Seen {
  const host = seen.mock.lastCall?.[0];
  if (!host) throw new Error('the probe has not rendered');
  return host;
}

const ocean = { id: 'ocean', slots: { Shell: Probe } };

const SETTINGS: ThemeSettingSpec[] = [
  { type: 'boolean', key: 'compact', label: 'Compact', default: false, hint: null },
  {
    type: 'select',
    key: 'accent',
    label: 'Accent',
    default: 'blue',
    hint: null,
    options: [
      { value: 'blue', label: 'Blue' },
      { value: 'red', label: 'Red' },
    ],
  },
];

beforeEach(() => {
  seen.mockClear();
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

describe('nav', () => {
  it('lists the entries of the navigation with the open page active', async () => {
    renderShell(ocean, { path: '/sessions' });
    await waitFor(() => {
      expect(seen).toHaveBeenCalled();
    });
    expect(last().nav).toEqual([
      { id: 'board', label: 'Board', href: '/board', active: false },
      { id: 'inbox', label: 'Inbox', href: '/inbox', active: false },
      { id: 'dashboard', label: 'Dashboard', href: '/dashboard', active: false },
      { id: 'questions', label: 'Questions', href: '/questions', active: false },
      { id: 'sessions', label: 'Sessions', href: '/sessions', active: true },
      { id: 'settings', label: 'Settings', href: '/settings', active: false },
    ]);
  });
});

describe('boards', () => {
  it('lists projects and boards and follows the choice of the app', async () => {
    const { queryClient } = renderShell(ocean);
    await waitFor(() => {
      expect(last().boards.loading).toBe(false);
    });
    const { boards } = last();
    expect(boards.projects).toEqual([
      { id: 'ARC', name: 'Arches' },
      { id: 'OPS', name: 'Operations' },
    ]);
    expect(boards.boards).toEqual([
      { id: BOARD_ID, project: 'ARC', name: 'Main' },
      { id: SIDE_BOARD_ID, project: 'ARC', name: 'Side' },
      { id: OPS_BOARD_ID, project: 'OPS', name: 'Ops' },
    ]);
    expect(boards.current).toEqual({ project: 'ARC', board: BOARD_ID });

    // a change the daemon reports reaches the host through the query cache
    act(() => {
      queryClient.setQueryData(boardKeys.list(), [
        { ...shellBoard(OPS_BOARD_ID, 'OPS', 'Ops renamed') },
      ]);
    });
    await waitFor(() => {
      expect(last().boards.boards).toEqual([
        { id: OPS_BOARD_ID, project: 'OPS', name: 'Ops renamed' },
      ]);
    });
  });

  it('chooses a board, or the first board of a project, and ignores anything else', async () => {
    renderShell(ocean);
    await waitFor(() => {
      expect(last().boards.current).not.toBeNull();
    });
    act(() => {
      last().boards.choose({ project: 'OPS', board: null });
    });
    expect(window.localStorage.getItem(BOARD_STORAGE_KEY)).toBe(OPS_BOARD_ID);
    await waitFor(() => {
      expect(last().boards.current).toEqual({ project: 'OPS', board: OPS_BOARD_ID });
    });
    act(() => {
      last().boards.choose({ project: 'ARC', board: SIDE_BOARD_ID });
    });
    await waitFor(() => {
      expect(last().boards.current).toEqual({ project: 'ARC', board: SIDE_BOARD_ID });
    });
    // a board of another project, an unknown board and an unknown project change nothing
    act(() => {
      last().boards.choose({ project: 'ARC', board: OPS_BOARD_ID });
      last().boards.choose({ project: 'ARC', board: 'unknown' });
      last().boards.choose({ project: 'NOPE', board: null });
    });
    expect(window.localStorage.getItem(BOARD_STORAGE_KEY)).toBe(SIDE_BOARD_ID);
  });
});

function shellBoard(id: string, projectKey: string, name: string) {
  return {
    id,
    projectId: '0190a0b0-0000-7000-8000-000000000002',
    projectKey,
    name,
    ownerId: '0190a0b0-0000-7000-8000-0000000000c1',
    ownerName: 'Ben',
    dispatchEnabled: false,
    openTickets: 0,
    createdAt: 1,
    updatedAt: 1,
    version: 1,
  };
}

describe('usage', () => {
  it('shows the limits of the first account', async () => {
    renderShell(ocean);
    await waitFor(() => {
      expect(last().usage.enabled).toBe(true);
    });
    expect(last().usage).toEqual({
      enabled: true,
      available: true,
      reason: null,
      fiveHour: { percent: 42, resetsAt: new Date(1_900_000_000_000).toISOString() },
      sevenDay: { percent: 7, resetsAt: null },
    });
  });

  it('is off without an account', async () => {
    renderShell(ocean, {
      routes: { 'GET /api/v1/accounts/usage': () => json(200, { items: [] }) },
    });
    await waitFor(() => {
      expect(last().boards.loading).toBe(false);
    });
    expect(last().usage).toEqual({
      enabled: false,
      available: false,
      reason: null,
      fiveHour: null,
      sevenDay: null,
    });
  });
});

describe('settings', () => {
  it('reads the chosen theme and switches only to a built-in theme', async () => {
    const { state } = renderShell(ocean);
    await waitFor(() => {
      expect(last().settings.values).toEqual({ theme: 'ocean' });
    });
    await act(async () => {
      await last().settings.update({ theme: 'light' });
    });
    expect(state.list.active).toBe('light');
  });

  it('refuses an installed theme, other fields and nothing to change', async () => {
    const api = renderShell(ocean);
    await waitFor(() => {
      expect(last().settings.values).not.toBeNull();
    });
    const put = () => fakeCalls('PUT /api/v1/settings/theme');
    await expect(last().settings.update({ theme: 'night-code' })).rejects.toThrow(
      'only to nullradix',
    );
    await expect(last().settings.update({ theme: 7 as never })).rejects.toThrow(
      'only to nullradix',
    );
    await last().settings.update({ claudeUsage: true } as never);
    await last().settings.update({});
    expect(put()).toHaveLength(0);
    expect(api.state.list.active).toBe('ocean');
  });
});

function fakeCalls(route: string): Request[] {
  const fetchMock = vi.mocked(fetch);
  return fetchMock.mock.calls
    .map(([request]) => request as Request)
    .filter((request) => `${request.method} ${new URL(request.url).pathname}` === route);
}

describe('theme settings', () => {
  const saved: { values: Record<string, unknown>; version: number } = { values: {}, version: 0 };
  const routes = {
    'GET /api/v1/themes/ocean/settings': () => json(200, saved),
    'PUT /api/v1/themes/ocean/settings': async (request: Request) => {
      const body = (await request.json()) as { values: Record<string, unknown>; version: number };
      saved.values = body.values;
      saved.version = body.version + 1;
      return json(200, saved);
    },
  };

  beforeEach(() => {
    saved.values = {};
    saved.version = 0;
  });

  it('gives the defaults of the manifest and saves one key after another', async () => {
    renderShell(ocean, { entry: { settings: SETTINGS }, routes });
    await waitFor(() => {
      expect(last().themeSettings.loading).toBe(false);
    });
    expect(last().themeSettings.values).toEqual({ compact: false, accent: 'blue' });
    await act(async () => {
      await Promise.all([
        last().themeSettings.set('compact', true),
        last().themeSettings.set('accent', 'red'),
      ]);
    });
    // the second save carries the version of the first, so nothing conflicts
    const bodies = await Promise.all(
      fakeCalls('PUT /api/v1/themes/ocean/settings').map((request) => request.clone().json()),
    );
    expect(bodies).toEqual([
      { values: { compact: true }, version: 0 },
      { values: { compact: true, accent: 'red' }, version: 1 },
    ]);
    await waitFor(() => {
      expect(last().themeSettings.values).toEqual({ compact: true, accent: 'red' });
    });
  });

  it('refuses a key the manifest does not declare', async () => {
    renderShell(ocean, { entry: { settings: SETTINGS }, routes });
    await waitFor(() => {
      expect(last().themeSettings.loading).toBe(false);
    });
    await expect(last().themeSettings.set('sneaky', true)).rejects.toThrow('declares no setting');
    expect(fakeCalls('PUT /api/v1/themes/ocean/settings')).toHaveLength(0);
  });

  it('is empty and asks nothing without declared settings', async () => {
    renderShell(ocean, { routes });
    await waitFor(() => {
      expect(last().boards.loading).toBe(false);
    });
    expect(last().themeSettings.values).toEqual({});
    expect(last().themeSettings.loading).toBe(false);
    expect(fakeCalls('GET /api/v1/themes/ocean/settings')).toHaveLength(0);
  });
});

describe('colorScheme', () => {
  it('shows the scheme the page shows and the theme', async () => {
    renderShell(ocean);
    await waitFor(() => {
      expect(last().scheme).toEqual({ scheme: 'dark', themeId: 'ocean', mode: 'ocean' });
    });
  });

  it('follows a light theme', async () => {
    renderShell(ocean, { entry: { colorScheme: 'light' } });
    await waitFor(() => {
      expect(last().scheme.scheme).toBe('light');
    });
  });
});

describe('navigate', () => {
  it('changes the route to a path of the app and to nothing else', async () => {
    const { router } = renderShell(ocean);
    await waitFor(() => {
      expect(seen).toHaveBeenCalled();
    });
    act(() => {
      last().navigate('https://evil.example/x');
      last().navigate('//evil.example/x');
      last().navigate('sessions');
    });
    expect(router.state.location.pathname).toBe('/inbox');
    act(() => {
      last().navigate('/sessions');
    });
    await waitFor(() => {
      expect(router.state.location.pathname).toBe('/sessions');
    });
  });
});

describe('without theme code', () => {
  it('asks the daemon for nothing the host would need', async () => {
    renderShell(ocean, { codeEnabled: false });
    await screen.findByRole('heading', { level: 1, name: 'Inbox' });
    expect(fakeCalls('GET /api/v1/projects')).toHaveLength(0);
    expect(fakeCalls('GET /api/v1/accounts/usage')).toHaveLength(0);
  });
});
