import {
  NavLink,
  Outlet,
  useBoards,
  useNav,
  type NavIconName,
  type PageProps,
  type ShellProps,
  type ThemeSlots,
} from '@nullradix/theme-sdk';
import { render, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { BOARD_STORAGE_KEY } from '../../features/board/boardChoice';
import { getThemeNotice } from '../../shared/lib/themeNotice';
import { BOARD_ID } from '../../test/boardServer';
import { SIDE_BOARD_ID, renderShell } from '../../test/renderShell';
import { ShellEscape } from './ShellEscape';

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

function Broken(): never {
  throw new Error('broken theme component');
}

// A shell from the nav, the content and the parts the host passes in.
function PartsShell({ pid, nav, content, parts }: ShellProps) {
  return (
    <div data-testid="theme-shell" data-pid={pid}>
      <header aria-label="Theme header">
        {parts.boardSwitcher}
        {parts.usage}
        {parts.status}
      </header>
      <nav aria-label="Theme navigation">{nav}</nav>
      {content}
    </div>
  );
}

// A shell from the SDK parts and the hooks of the host only.
function SdkShell() {
  const nav = useNav();
  const boards = useBoards();
  return (
    <div data-testid="sdk-shell">
      <p>{`board:${boards.current?.board ?? 'none'}`}</p>
      <button
        type="button"
        onClick={() => {
          boards.choose({ project: 'ARC', board: SIDE_BOARD_ID });
        }}
      >
        Side board
      </button>
      <nav aria-label="Theme navigation">
        {nav.map((item) => (
          <NavLink key={item.id} id={item.id} />
        ))}
      </nav>
      <main>
        <Outlet />
      </main>
    </div>
  );
}

const ocean = (slots: ThemeSlots) => ({ id: 'ocean', slots });

describe('the slot Shell', () => {
  it('replaces the built-in shell and gets the nav, the content and the parts', async () => {
    renderShell(ocean({ Shell: PartsShell }));
    const shell = await screen.findByTestId('theme-shell');
    expect(document.querySelector('.shell-sidebar')).toBeNull();
    expect(screen.queryByRole('navigation', { name: 'Main navigation' })).not.toBeInTheDocument();
    const nav = within(screen.getByRole('navigation', { name: 'Theme navigation' }));
    expect(nav.getAllByRole('link').map((link) => link.textContent)).toEqual([
      'Board',
      'Inbox',
      'Dashboard',
      'Questions',
      'Sessions',
      'Settings',
    ]);
    // the page of the route is the content
    expect(within(shell).getByRole('heading', { level: 1, name: 'Inbox' })).toBeInTheDocument();
    // the project key of the chosen board, once the boards are known
    await waitFor(() => {
      expect(shell).toHaveAttribute('data-pid', 'ARC');
    });
    const header = within(screen.getByLabelText('Theme header'));
    expect(await header.findByRole('combobox', { name: 'Board' })).toBeInTheDocument();
    expect(await header.findByRole('progressbar')).toHaveValue(42);
    expect(header.getByRole('button', { name: /Daemon status/ })).toBeInTheDocument();
  });

  it('builds a whole shell from the SDK parts and the host hooks', async () => {
    renderShell(ocean({ Shell: SdkShell }));
    expect(await screen.findByText(`board:${BOARD_ID}`)).toBeVisible();
    expect(screen.getByRole('link', { name: 'Inbox' })).toHaveAttribute('aria-current', 'page');
    await userEvent.click(screen.getByRole('link', { name: 'Sessions' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Sessions' })).toBeVisible();
    expect(screen.getByRole('link', { name: 'Sessions' })).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('link', { name: 'Inbox' })).not.toHaveAttribute('aria-current');
    // a board chosen by the theme is the board the app remembers
    await userEvent.click(screen.getByRole('button', { name: 'Side board' }));
    expect(screen.getByText(`board:${SIDE_BOARD_ID}`)).toBeVisible();
    expect(window.localStorage.getItem(BOARD_STORAGE_KEY)).toBe(SIDE_BOARD_ID);
  });

  it('falls back to the built-in shell and tells the person once when it throws', async () => {
    renderShell(ocean({ Shell: Broken }));
    expect(await screen.findByRole('navigation', { name: 'Main navigation' })).toBeVisible();
    expect(screen.getByRole('heading', { level: 1, name: 'Inbox' })).toBeVisible();
    expect(getThemeNotice()?.message).toContain('"ocean"');
    expect(document.querySelectorAll('.theme-notice')).toHaveLength(1);
  });

  it('is not used while the code switch of the theme is off', () => {
    renderShell(ocean({ Shell: PartsShell }), { codeEnabled: false });
    expect(screen.queryByTestId('theme-shell')).not.toBeInTheDocument();
    expect(screen.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
    expect(screen.queryByRole('navigation', { name: 'Theme escape' })).not.toBeInTheDocument();
  });
});

describe('the way out of a theme shell', () => {
  it('shows the escape bar only while a theme shell is shown', () => {
    renderShell(ocean({}));
    expect(screen.queryByRole('navigation', { name: 'Theme escape' })).not.toBeInTheDocument();
  });

  it('keeps safe mode and the settings reachable under a shell that renders nothing', async () => {
    const { router } = renderShell(ocean({ Shell: () => null }));
    const escape = within(await screen.findByRole('navigation', { name: 'Theme escape' }));
    expect(screen.queryByRole('heading', { level: 1 })).not.toBeInTheDocument();
    expect(escape.getByRole('button', { name: 'Safe mode' })).toBeEnabled();
    await userEvent.click(escape.getByRole('link', { name: 'Settings' }));
    expect(router.state.location.pathname).toBe('/settings');
    // the settings show in the built-in shell, where the code switch of the theme is
    expect(await screen.findByRole('heading', { level: 1, name: 'Settings' })).toBeVisible();
    expect(screen.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
    expect(screen.queryByRole('navigation', { name: 'Theme escape' })).not.toBeInTheDocument();
    // leaving the settings through the built-in navigation returns to the theme shell
    await userEvent.click(screen.getByRole('link', { name: 'Inbox' }));
    expect(await screen.findByRole('navigation', { name: 'Theme escape' })).toBeVisible();
  });

  it('opens a link to the settings from the theme itself in the theme shell', async () => {
    renderShell(ocean({ Shell: SdkShell }));
    const nav = within(await screen.findByRole('navigation', { name: 'Theme navigation' }));
    await userEvent.click(nav.getByRole('link', { name: 'Settings' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Settings' })).toBeVisible();
    expect(screen.getByTestId('sdk-shell')).toBeVisible();
    expect(screen.getByRole('navigation', { name: 'Theme escape' })).toBeVisible();
  });

  it('reloads in safe mode from the escape bar', async () => {
    const assign = vi.fn();
    render(
      <MemoryRouter>
        <ShellEscape assign={assign} />
      </MemoryRouter>,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Safe mode' }));
    expect(assign).toHaveBeenCalledTimes(1);
    expect(String(assign.mock.calls[0]?.[0])).toContain('safe-theme');
  });
});

describe('the slot NavIcon', () => {
  it('replaces the icons of the built-in navigation', async () => {
    const names: NavIconName[] = [];
    renderShell(
      ocean({
        NavIcon: ({ name }) => {
          names.push(name);
          return <i data-icon={name} />;
        },
      }),
    );
    const nav = within(await screen.findByRole('navigation', { name: 'Main navigation' }));
    expect(nav.getAllByRole('link')).toHaveLength(6);
    expect(document.querySelector('a[href="/board"] i[data-icon="board"]')).not.toBeNull();
    expect(document.querySelector('a[href="/settings"] i[data-icon="settings"]')).not.toBeNull();
    expect(document.querySelector('svg.nav-icon')).toBeNull();
  });

  it('falls back to the built-in icons when it throws', () => {
    renderShell(ocean({ NavIcon: Broken }));
    expect(document.querySelectorAll('.nav svg.nav-icon')).toHaveLength(6);
    expect(getThemeNotice()?.message).toContain('"ocean"');
  });
});

describe('page slots', () => {
  function Page({ pid }: PageProps) {
    return <h1>{`Dashboard of ${pid}`}</h1>;
  }

  it('replaces the dashboard and passes the project key', async () => {
    renderShell(ocean({ 'page:dashboard': Page }), { path: '/dashboard' });
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Dashboard of ARC' }),
    ).toBeVisible();
    // the shell around it stays the built-in one
    expect(screen.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
  });

  it('shows the built-in page without the slot', () => {
    renderShell(ocean({}), { path: '/dashboard' });
    expect(screen.getByRole('heading', { level: 1, name: 'Dashboard' })).toBeVisible();
  });

  it('falls back to the built-in page and tells the person once when the page throws', () => {
    renderShell(ocean({ 'page:dashboard': Broken }), { path: '/dashboard' });
    expect(screen.getByRole('heading', { level: 1, name: 'Dashboard' })).toBeVisible();
    expect(getThemeNotice()?.message).toContain('"ocean"');
    expect(document.querySelectorAll('.theme-notice')).toHaveLength(1);
  });

  it('changes with the route like the built-in page', async () => {
    renderShell(ocean({ 'page:dashboard': Page }), { path: '/inbox' });
    await userEvent.click(screen.getByRole('link', { name: 'Dashboard' }));
    expect(await screen.findByRole('heading', { level: 1, name: /^Dashboard of/ })).toBeVisible();
    await userEvent.click(screen.getByRole('link', { name: 'Inbox' }));
    expect(screen.getByRole('heading', { level: 1, name: 'Inbox' })).toBeVisible();
  });
});
