import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  useBoards,
  useColorScheme,
  useNav,
  useNavigate,
  useOpenSearch,
  useSettings,
  useUsage,
} from '../src/index.js';
import { fakeHost, renderWithHost } from './fakeHost.js';

afterEach(cleanup);

describe('SDK hooks', () => {
  it('useNav returns the navigation entries', () => {
    function Nav() {
      return (
        <ul>
          {useNav().map((n) => (
            <li
              key={n.id}
            >{`${n.label}|${n.href}|${String(n.active)}|${String(n.badge ?? '')}`}</li>
          ))}
        </ul>
      );
    }
    renderWithHost(fakeHost(), <Nav />);
    expect(screen.getByText('Board|/p/p1/board|true|')).toBeTruthy();
    expect(screen.getByText('My tasks|/p/p1/inbox|false|2')).toBeTruthy();
  });

  it('useBoards returns the current board and chooses another', () => {
    const host = fakeHost();
    function Boards() {
      const boards = useBoards();
      const label = `${boards.current?.project ?? ''}/${boards.boards[0]?.name ?? ''}`;
      return (
        <button
          onClick={() => {
            boards.choose({ project: 'ARC', board: 'b1' });
          }}
        >
          {label}
        </button>
      );
    }
    renderWithHost(host, <Boards />);
    fireEvent.click(screen.getByRole('button', { name: 'ARC/Sprint' }));
    expect(host.boards.choose).toHaveBeenCalledWith({ project: 'ARC', board: 'b1' });
  });

  it('useUsage returns the usage', () => {
    function Usage() {
      const usage = useUsage();
      return <span>{`${String(usage.enabled)}:${String(usage.fiveHour?.percent)}`}</span>;
    }
    renderWithHost(fakeHost(), <Usage />);
    expect(screen.getByText('true:42')).toBeTruthy();
  });

  it('useSettings returns the settings and changes them', () => {
    const host = fakeHost();
    function Settings() {
      const settings = useSettings();
      return (
        <button onClick={() => void settings.update({ theme: 'light' })}>
          {`theme:${String(settings.values?.theme)}`}
        </button>
      );
    }
    renderWithHost(host, <Settings />);
    fireEvent.click(screen.getByRole('button', { name: 'theme:dark' }));
    expect(host.settings.update).toHaveBeenCalledWith({ theme: 'light' });
  });

  it('useColorScheme returns the color scheme', () => {
    function Scheme() {
      const scheme = useColorScheme();
      return <span>{`${scheme.scheme}/${scheme.themeId}`}</span>;
    }
    renderWithHost(fakeHost(), <Scheme />);
    expect(screen.getByText('dark/dark')).toBeTruthy();
  });

  it('useNavigate changes the route through the host', () => {
    const host = fakeHost();
    function Go() {
      const navigate = useNavigate();
      return (
        <button
          onClick={() => {
            navigate('/p/p1/docs');
          }}
        >
          go
        </button>
      );
    }
    renderWithHost(host, <Go />);
    fireEvent.click(screen.getByRole('button', { name: 'go' }));
    expect(host.navigate).toHaveBeenCalledWith('/p/p1/docs');
  });

  it('useOpenSearch opens the search through the host', () => {
    const host = fakeHost();
    function Find() {
      const openSearch = useOpenSearch();
      return <button onClick={openSearch}>search</button>;
    }
    renderWithHost(host, <Find />);
    fireEvent.click(screen.getByRole('button', { name: 'search' }));
    expect(host.openSearch).toHaveBeenCalled();
  });

  it.each([
    ['useNav', useNav],
    ['useBoards', useBoards],
    ['useUsage', useUsage],
    ['useSettings', useSettings],
    ['useColorScheme', useColorScheme],
    ['useNavigate', useNavigate],
    ['useOpenSearch', useOpenSearch],
  ] as const)('%s throws a clear error without the host provider', (name, hook) => {
    function Probe() {
      hook();
      return null;
    }
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    expect(() => render(<Probe />)).toThrow(
      new RegExp(`${name} needs the ThemeHostProvider of the NULLRADIX host`),
    );
  });
});

describe('SDK boundary', () => {
  it('no source below src imports from the web app', () => {
    const dir = join(import.meta.dirname, '..', 'src');
    for (const file of readdirSync(dir)) {
      const text = readFileSync(join(dir, file), 'utf8');
      expect(text, file).not.toMatch(/from\s+['"][^'"]*(\.\.\/web|@nullradix\/web)/);
    }
  });
});
