import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { FakeEventSource } from '../../test/eventSource';
import { fakeApi, json, problem, signedIn, signedOut } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';

const gateHeading = { level: 1, name: 'Open NULLRADIX from your terminal' } as const;

describe('session gate', () => {
  it('asks for nullradix open while the daemon answers 401', async () => {
    fakeApi({ 'GET /api/v1/auth/me': signedOut });
    renderApp('/board');
    expect(await screen.findByRole('heading', gateHeading)).toBeInTheDocument();
    expect(screen.getByText('nullradix open')).toBeInTheDocument();
    expect(screen.queryByRole('navigation', { name: 'Main navigation' })).not.toBeInTheDocument();
    expect(await screen.findByText('Daemon running, version 0.1.0')).toBeInTheDocument();
    expect(FakeEventSource.instances).toHaveLength(0);
  });

  it('shows the shell and opens the change stream with a session', async () => {
    fakeApi();
    renderApp('/board');
    expect(await screen.findByRole('heading', { level: 1, name: 'Board' })).toBeInTheDocument();
    await screen.findByText('Daemon running, version 0.1.0');
    expect(FakeEventSource.latest().url).toBe('/api/v1/events');
    expect(screen.queryByRole('heading', gateHeading)).not.toBeInTheDocument();
  });

  it('shows the shell when the daemon cannot be reached', async () => {
    fakeApi({ 'GET /api/v1/auth/me': () => json(503, 'down') });
    renderApp('/board');
    expect(await screen.findByRole('heading', { level: 1, name: 'Board' })).toBeInTheDocument();
    expect(FakeEventSource.instances).toHaveLength(0);
  });

  it('falls back to the gate when another request answers 401', async () => {
    let sessionChecks = 0;
    const api = fakeApi({
      'GET /api/v1/auth/me': (request) => {
        sessionChecks += 1;
        return sessionChecks === 1 ? signedIn(request) : signedOut(request);
      },
      'GET /api/v1/projects': () => problem(401, 'invalid-session', 'session expired'),
    });
    renderApp('/settings');
    expect(await screen.findByRole('heading', gateHeading)).toBeInTheDocument();
    expect(api.requests('GET /api/v1/auth/me')).toHaveLength(2);
  });
});
