import { screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { renderApp } from '../../test/renderApp';

function daemonOffline() {
  return vi.fn(() => Promise.reject(new TypeError('offline')));
}

describe('AppLayout', () => {
  it('renders the main navigation in order', () => {
    vi.stubGlobal('fetch', daemonOffline());
    renderApp('/board');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    const labels = within(nav)
      .getAllByRole('link')
      .map((link) => link.textContent);
    expect(labels).toEqual(['Board', 'Inbox', 'Dashboard', 'Questions', 'Sessions', 'Settings']);
  });

  it('redirects the root to the board', async () => {
    vi.stubGlobal('fetch', daemonOffline());
    renderApp('/');
    expect(await screen.findByRole('heading', { level: 1, name: 'Board' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Board' })).toHaveClass('active');
  });

  it('navigates to a placeholder page', async () => {
    vi.stubGlobal('fetch', daemonOffline());
    renderApp('/board');
    await userEvent.click(screen.getByRole('link', { name: 'Inbox' }));
    expect(screen.getByRole('heading', { level: 1, name: 'Inbox' })).toBeInTheDocument();
    expect(screen.getByText('Coming soon')).toBeInTheDocument();
  });
});
