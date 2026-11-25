import { render, screen, waitFor } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { AppProviders } from '../../app/providers';
import { createQueryClient } from '../../app/queryClient';
import { liveRegistry } from '../../app/liveRegistry';
import { statusDto } from '../../test/dispatcherFixture';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { DispatcherIndicator } from './DispatcherIndicator';
import { dispatcherKeys } from './queries';
import type { DispatcherPhase } from './statusQueries';

function renderIndicator(handlers: Parameters<typeof fakeApi>[0] = {}) {
  const api = fakeApi(handlers);
  render(
    <AppProviders queryClient={createQueryClient({ retry: false })}>
      <DispatcherIndicator />
    </AppProviders>,
  );
  return api;
}

describe('DispatcherIndicator', () => {
  it.each<[DispatcherPhase, string]>([
    ['off', 'Dispatcher: Off'],
    ['idle', 'Dispatcher: Idle'],
    ['running', 'Dispatcher: Running 1/2'],
    ['paused', 'Dispatcher: Paused'],
    ['outside-window', 'Dispatcher: Outside window'],
  ])('labels the %s state', async (state, label) => {
    renderIndicator({
      'GET /api/v1/dispatcher': () =>
        json(200, statusDto({ state, active: 1, wip: 2, message: 'Because.' })),
    });
    const button = await screen.findByRole('button', { name: label });
    expect(button).toHaveAttribute('title', 'Because.');
  });

  it('pauses for an hour', async () => {
    const api = renderIndicator({
      'GET /api/v1/dispatcher': () => json(200, statusDto()),
      'POST /api/v1/dispatcher/pause': () => json(200, statusDto({ state: 'paused' })),
    });
    await userEvent.click(await screen.findByRole('button', { name: /Dispatcher:/ }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Pause 1 h' }));
    await waitFor(() => {
      expect(api.requests('POST /api/v1/dispatcher/pause')).toHaveLength(1);
    });
    expect(await api.requests('POST /api/v1/dispatcher/pause')[0]?.json()).toEqual({
      minutes: 60,
    });
  });

  it('resumes a paused dispatcher', async () => {
    const api = renderIndicator({
      'GET /api/v1/dispatcher': () => json(200, statusDto({ state: 'paused' })),
      'POST /api/v1/dispatcher/resume': () => json(200, statusDto()),
    });
    await userEvent.click(await screen.findByRole('button', { name: /Dispatcher:/ }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Resume' }));
    await waitFor(() => {
      expect(api.requests('POST /api/v1/dispatcher/resume')).toHaveLength(1);
    });
  });

  it('enables an off dispatcher', async () => {
    const api = renderIndicator({
      'GET /api/v1/dispatcher': () => json(200, statusDto({ enabled: false, state: 'off' })),
      'PATCH /api/v1/dispatcher': () => json(200, statusDto()),
    });
    await userEvent.click(await screen.findByRole('button', { name: /Dispatcher:/ }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Enable' }));
    await waitFor(() => {
      expect(api.requests('PATCH /api/v1/dispatcher')).toHaveLength(1);
    });
    expect(await api.requests('PATCH /api/v1/dispatcher')[0]?.json()).toEqual({ enabled: true });
  });

  it('runs a pass now', async () => {
    const api = renderIndicator({
      'GET /api/v1/dispatcher': () => json(200, statusDto()),
      'POST /api/v1/dispatcher/passes': () =>
        json(200, { at: 1, idle: null, started: [], queued: [], released: [], errors: [] }),
    });
    await userEvent.click(await screen.findByRole('button', { name: /Dispatcher:/ }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Run pass now' }));
    await waitFor(() => {
      expect(api.requests('POST /api/v1/dispatcher/passes')).toHaveLength(1);
    });
  });

  it('shows the error of a failed request', async () => {
    renderIndicator({
      'GET /api/v1/dispatcher': () => json(200, statusDto()),
      'POST /api/v1/dispatcher/passes': () => problem(409, 'busy', 'A pass is already running.'),
    });
    await userEvent.click(await screen.findByRole('button', { name: /Dispatcher:/ }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Run pass now' }));
    expect(await screen.findByText('A pass is already running.')).toBeInTheDocument();
  });
});

describe('the dispatcher status live keys', () => {
  it('refreshes the status on dispatcher state, run and ticket changes', () => {
    for (const entity of ['dispatcher-state', 'run', 'ticket']) {
      expect(liveRegistry[entity]).toContainEqual(dispatcherKeys.status());
    }
  });
});
