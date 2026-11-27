import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  BoardSwitcher,
  ConnectionStatus,
  NavLink,
  Outlet,
  SearchButton,
  UsageMeter,
  useShellParts,
} from '../src/index.js';
import type { ThemeHost } from '../src/index.js';
import { fakeHost, renderWithHost } from './fakeHost.js';

afterEach(cleanup);

// Host with recognizable parts and an observable route change
function partsHost(): ThemeHost {
  return fakeHost({
    nav: [
      { id: 'board', label: 'Board', href: '/p/p1/board', active: true },
      { id: 'inbox', label: 'My tasks', href: '/p/p1/inbox', active: false, badge: 3 },
    ],
    parts: {
      search: <span>part-search</span>,
      boardSwitcher: <span>part-board</span>,
      usage: <span>part-usage</span>,
      navItems: { board: <span>part-nav-board</span> },
      outlet: <span>part-page</span>,
      connection: <span>part-banner</span>,
      status: <span>part-status</span>,
    },
  });
}

describe('NavLink', () => {
  it('shows label, target and aria-current of the active entry', () => {
    renderWithHost(partsHost(), <NavLink id="board" />);
    const link = screen.getByRole('link', { name: 'Board' });
    expect(link.getAttribute('href')).toBe('/p/p1/board');
    expect(link.getAttribute('aria-current')).toBe('page');
  });

  it('shows the counter and no aria-current on an inactive entry', () => {
    renderWithHost(partsHost(), <NavLink id="inbox" />);
    const link = screen.getByRole('link', { name: /My tasks/ });
    expect(link.getAttribute('aria-current')).toBeNull();
    expect(link.textContent).toContain('3');
  });

  it('a plain click changes the route through the host, a Ctrl click does not', () => {
    const host = partsHost();
    renderWithHost(host, <NavLink id="inbox" />);
    const link = screen.getByRole('link', { name: /My tasks/ });
    fireEvent.click(link, { ctrlKey: true });
    expect(host.navigate).not.toHaveBeenCalled();
    fireEvent.click(link);
    expect(host.navigate).toHaveBeenCalledWith('/p/p1/inbox');
  });

  it('renders its own look with children as a function', () => {
    renderWithHost(
      partsHost(),
      <NavLink id="board">{(item) => <b>{`custom:${item.label}`}</b>}</NavLink>,
    );
    expect(screen.getByRole('link', { name: 'custom:Board' })).toBeTruthy();
  });

  it('passes role through and calls onClick before the route change', () => {
    const host = partsHost();
    const onClick = vi.fn();
    renderWithHost(host, <NavLink id="inbox" role="menuitem" onClick={onClick} />);
    fireEvent.click(screen.getByRole('menuitem', { name: /My tasks/ }));
    expect(onClick).toHaveBeenCalled();
    expect(host.navigate).toHaveBeenCalledWith('/p/p1/inbox');
  });

  it('renders nothing for an unknown id', () => {
    const { container } = renderWithHost(partsHost(), <NavLink id="docs" />);
    expect(container.innerHTML).toBe('');
  });
});

describe('parts of the host', () => {
  it('render the matching element of the host', () => {
    function Parts() {
      return <>{useShellParts().navItems.board}</>;
    }
    renderWithHost(
      partsHost(),
      <>
        <BoardSwitcher />
        <SearchButton />
        <UsageMeter />
        <Outlet />
        <ConnectionStatus />
        <Parts />
      </>,
    );
    const texts = ['part-board', 'part-search', 'part-usage', 'part-page', 'part-status'];
    for (const text of [...texts, 'part-nav-board']) {
      expect(screen.getByText(text)).toBeTruthy();
    }
  });

  it.each([
    ['NavLink', () => <NavLink id="board" />],
    ['BoardSwitcher', () => <BoardSwitcher />],
    ['SearchButton', () => <SearchButton />],
    ['UsageMeter', () => <UsageMeter />],
    ['Outlet', () => <Outlet />],
    ['ConnectionStatus', () => <ConnectionStatus />],
  ] as const)('%s throws a clear error without the host provider', (name, ui) => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    expect(() => render(ui())).toThrow(
      new RegExp(`${name} needs the ThemeHostProvider of the NULLRADIX host`),
    );
  });
});
