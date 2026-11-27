import { render } from '@testing-library/react';
import type { ReactNode } from 'react';
import { vi } from 'vitest';
import { ThemeHostProvider } from '../src/index.js';
import type { ThemeHost } from '../src/index.js';

/** Host with fixed values and observable actions */
export function fakeHost(overrides: Partial<ThemeHost> = {}): ThemeHost {
  return {
    nav: [
      { id: 'board', label: 'Board', href: '/p/p1/board', active: true },
      { id: 'inbox', label: 'My tasks', href: '/p/p1/inbox', active: false, badge: 2 },
    ],
    boards: {
      current: { project: 'ARC', board: 'b1' },
      projects: [{ id: 'ARC', name: 'Arches' }],
      boards: [{ id: 'b1', project: 'ARC', name: 'Sprint' }],
      loading: false,
      choose: vi.fn(),
    },
    usage: {
      enabled: true,
      available: true,
      reason: null,
      fiveHour: { percent: 42, resetsAt: null },
      sevenDay: null,
    },
    settings: {
      values: { theme: 'dark' },
      loading: false,
      update: vi.fn(() => Promise.resolve()),
    },
    themeSettings: { values: {}, loading: false, set: vi.fn(() => Promise.resolve()) },
    colorScheme: { scheme: 'dark', themeId: 'dark', mode: 'dark' },
    navigate: vi.fn(),
    openSearch: vi.fn(),
    parts: {
      search: null,
      boardSwitcher: null,
      usage: null,
      navItems: {},
      outlet: null,
      connection: null,
      status: null,
    },
    ...overrides,
  };
}

export function renderWithHost(host: ThemeHost, ui: ReactNode) {
  return render(<ThemeHostProvider value={host}>{ui}</ThemeHostProvider>);
}
