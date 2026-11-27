import type { ThemeDefinition } from '@nullradix/theme-sdk';
import { RouterProvider, createMemoryRouter } from 'react-router';
import { routes } from '../app/routes';
import type { Theme } from '../shared/query/themes';
import { ThemeNotice } from '../shared/ui/ThemeNotice';
import { board } from './boardServer';
import { json, type Handler } from './fakeApi';
import { renderThemed } from './renderThemed';

export const SIDE_BOARD_ID = '0190a0b0-0000-7000-8000-0000000000b2';
export const OPS_BOARD_ID = '0190a0b0-0000-7000-8000-0000000000b3';

function project(key: string, name: string) {
  return {
    id: `0190a0b0-0000-7000-8000-00000000${key === 'ARC' ? '0001' : '0002'}`,
    key,
    name,
    createdAt: 1,
    updatedAt: 1,
    version: 1,
  };
}

// Two projects with three boards, and one account with its limits.
export const shellApi: Record<string, Handler> = {
  'GET /api/v1/boards': () =>
    json(200, {
      items: [
        board(),
        board({ id: SIDE_BOARD_ID, name: 'Side' }),
        board({ id: OPS_BOARD_ID, projectKey: 'OPS', name: 'Ops' }),
      ],
    }),
  'GET /api/v1/projects': () =>
    json(200, { items: [project('ARC', 'Arches'), project('OPS', 'Operations')] }),
  'GET /api/v1/accounts/usage': () =>
    json(200, {
      items: [
        {
          accountId: '0190a0b0-0000-7000-8000-0000000000a1',
          slug: 'max',
          loggedIn: true,
          folder: '/accounts/max',
          usage: {
            available: true,
            reason: null,
            plan: 'max',
            fiveHour: { percent: 42, resetsAt: 1_900_000_000_000 },
            sevenDay: { percent: 7, resetsAt: null },
            fetchedAt: 1,
          },
        },
      ],
    }),
};

// The whole app on a route, under the real providers, with a theme module registered.
export function renderShell(
  definition: ThemeDefinition,
  options: {
    path?: string;
    codeEnabled?: boolean;
    entry?: Partial<Theme>;
    routes?: Record<string, Handler>;
  } = {},
) {
  const router = createMemoryRouter(routes, { initialEntries: [options.path ?? '/inbox'] });
  const view = renderThemed(
    <>
      <RouterProvider router={router} />
      <ThemeNotice />
    </>,
    definition,
    {
      ...(options.codeEnabled !== undefined && { codeEnabled: options.codeEnabled }),
      ...(options.entry && { entry: options.entry }),
      routes: { ...shellApi, ...options.routes },
    },
  );
  return { ...view, router };
}
