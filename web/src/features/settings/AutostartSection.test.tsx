import { screen, waitFor } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import type { Autostart } from '../../shared/query/autostart';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import { autostartHint } from './AutostartSection';

const ROUTE = '/api/v1/settings/autostart';
const LABEL = 'Start NULLRADIX when I log in';

function server(initial: Autostart = { available: true, enabled: false }) {
  let current = initial;
  return fakeApi({
    [`GET ${ROUTE}`]: () => json(200, current),
    [`PUT ${ROUTE}`]: async (request) => {
      const { enabled } = (await request.json()) as { enabled: boolean };
      current = { available: true, enabled };
      return json(200, current);
    },
  });
}

describe('AutostartSection', () => {
  it('shows the switch off until it is turned on', async () => {
    server();
    renderApp('/settings');
    const toggle = await screen.findByRole('switch', { name: LABEL });
    await waitFor(() => {
      expect(toggle).toBeEnabled();
    });
    expect(toggle).not.toBeChecked();
    expect(screen.getByText('Off: NULLRADIX starts when you open it.')).toBeInTheDocument();
  });

  it('turns it on and sends the choice', async () => {
    const api = server();
    renderApp('/settings');
    const toggle = await screen.findByRole('switch', { name: LABEL });
    await waitFor(() => {
      expect(toggle).toBeEnabled();
    });
    await userEvent.click(toggle);
    await waitFor(() => {
      expect(toggle).toBeChecked();
    });
    expect(await api.requests(`PUT ${ROUTE}`)[0]?.clone().json()).toEqual({ enabled: true });
  });

  it('disables the switch on a host without a login', async () => {
    server({ available: false, enabled: false });
    renderApp('/settings');
    const toggle = await screen.findByRole('switch', { name: LABEL });
    expect(
      await screen.findByText(autostartHint({ available: false, enabled: false })),
    ).toBeInTheDocument();
    expect(toggle).toBeDisabled();
  });

  it('shows why the entry could not be changed', async () => {
    fakeApi({
      [`GET ${ROUTE}`]: () => json(200, { available: true, enabled: false }),
      [`PUT ${ROUTE}`]: () =>
        problem(500, 'autostart-failed', 'cannot change the login entry: denied'),
    });
    renderApp('/settings');
    const toggle = await screen.findByRole('switch', { name: LABEL });
    await waitFor(() => {
      expect(toggle).toBeEnabled();
    });
    await userEvent.click(toggle);
    expect(
      await screen.findByText('The system refused the login entry; the daemon log says why.'),
    ).toBeInTheDocument();
    expect(toggle).not.toBeChecked();
  });
});
