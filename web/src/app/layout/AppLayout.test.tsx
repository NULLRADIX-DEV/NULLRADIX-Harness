import { screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { renderApp } from '../../test/renderApp';

function daemonUp() {
  const body = JSON.stringify({ status: 'ok', version: '0.1.0', apiVersion: 1 });
  return vi.fn(() =>
    Promise.resolve(
      new Response(body, { status: 200, headers: { 'content-type': 'application/json' } }),
    ),
  );
}

describe('AppLayout', () => {
  it('renders the main navigation in order', () => {
    vi.stubGlobal('fetch', daemonUp());
    renderApp('/board');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    const labels = within(nav)
      .getAllByRole('link')
      .map((link) => link.textContent);
    expect(labels).toEqual(['Board', 'Inbox', 'Dashboard', 'Questions', 'Sessions', 'Settings']);
  });

  it('redirects the root to the board', async () => {
    vi.stubGlobal('fetch', daemonUp());
    renderApp('/');
    expect(await screen.findByRole('heading', { level: 1, name: 'Board' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Board' })).toHaveClass('active');
  });

  it('navigates to a placeholder page', async () => {
    vi.stubGlobal('fetch', daemonUp());
    renderApp('/board');
    await userEvent.click(screen.getByRole('link', { name: 'Sessions' }));
    expect(screen.getByRole('heading', { level: 1, name: 'Sessions' })).toBeInTheDocument();
    expect(screen.getByText('Coming soon')).toBeInTheDocument();
  });

  it('shows the daemon as running when health answers', async () => {
    const fetch = daemonUp();
    vi.stubGlobal('fetch', fetch);
    renderApp('/board');
    expect(await screen.findByText('Daemon running, version 0.1.0')).toBeInTheDocument();
    expect(screen.getByRole('status')).not.toHaveClass('off');
    const request = fetch.mock.calls[0] as unknown as [Request];
    expect(new URL(request[0].url).pathname).toBe('/health');
  });

  it('shows the daemon as unreachable when fetch fails', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.reject(new TypeError('connection refused'))),
    );
    renderApp('/board');
    expect(await screen.findByText('Daemon unreachable')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveClass('off');
  });

  it('shows the daemon as unreachable on an error status', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(new Response(null, { status: 503 }))),
    );
    renderApp('/board');
    expect(await screen.findByText('Daemon unreachable')).toBeInTheDocument();
  });
});
