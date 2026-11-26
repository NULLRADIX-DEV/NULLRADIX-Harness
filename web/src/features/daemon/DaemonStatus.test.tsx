import { screen } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { renderApp } from '../../test/renderApp';
import { healthKeys } from './queries';

function health(apiVersion: number) {
  const body = JSON.stringify({ status: 'ok', version: '0.1.0', apiVersion });
  return vi.fn((request: Request) => {
    const found = new URL(request.url).pathname === '/api/v1/health';
    return Promise.resolve(
      new Response(found ? body : '{}', {
        status: found ? 200 : 404,
        headers: { 'content-type': 'application/json' },
      }),
    );
  });
}

function statusButton(): HTMLElement {
  return screen.getByRole('button', { name: /^Daemon status: / });
}

describe('DaemonStatus', () => {
  it('uses a stable query key', () => {
    expect(healthKeys.all).toEqual(['daemon', 'health']);
  });

  it('shows the daemon as running when health answers', async () => {
    const fetch = health(1);
    vi.stubGlobal('fetch', fetch);
    renderApp('/board');
    await screen.findByText('Daemon running, version 0.1.0');
    expect(screen.getByRole('status')).toHaveTextContent('Daemon running, version 0.1.0');
    expect(statusButton()).toHaveAccessibleName('Daemon status: running');
    expect(statusButton()).toHaveClass('up');
    const [request] = fetch.mock.calls[0] as unknown as [Request];
    expect(new URL(request.url).pathname).toBe('/api/v1/health');
  });

  it('flags a daemon with another API version as incompatible', async () => {
    vi.stubGlobal('fetch', health(2));
    renderApp('/board');
    await screen.findByText('Incompatible daemon: API version 2, expected 1');
    expect(screen.getByRole('status')).toHaveTextContent('Incompatible daemon');
    expect(statusButton()).toHaveAccessibleName('Daemon status: incompatible');
    expect(statusButton()).toHaveClass('incompatible');
  });

  it('shows the daemon as unreachable when fetch fails', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.reject(new TypeError('connection refused'))),
    );
    renderApp('/board');
    expect(await screen.findByText('Daemon unreachable')).toBeInTheDocument();
    expect(statusButton()).toHaveClass('down');
  });

  it('shows the daemon as unreachable on a problem response', async () => {
    const problem = JSON.stringify({
      type: 'about:blank',
      title: 'Forbidden',
      status: 403,
      detail: 'host not allowed',
      code: 'host-not-allowed',
    });
    vi.stubGlobal(
      'fetch',
      vi.fn(() =>
        Promise.resolve(
          new Response(problem, {
            status: 403,
            headers: { 'content-type': 'application/problem+json' },
          }),
        ),
      ),
    );
    renderApp('/board');
    expect(await screen.findByText('Daemon unreachable')).toBeInTheDocument();
  });

  it('is reachable by keyboard and described by its label', async () => {
    vi.stubGlobal('fetch', health(1));
    renderApp('/board');
    await screen.findByText('Daemon running, version 0.1.0');
    await userEvent.tab();
    expect(statusButton()).toHaveFocus();
    expect(statusButton()).toHaveAccessibleDescription('Daemon running, version 0.1.0');
  });
});
