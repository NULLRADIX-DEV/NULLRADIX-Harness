import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { createQueryClient } from '../../app/queryClient';
import { fakeApi, json } from '../../test/fakeApi';
import { usageQuery } from './queries';
import { UsageMeter } from './UsageMeter';

function status(usage: object) {
  return {
    accountId: '0190a0b0-0000-7000-8000-0000000000a1',
    slug: 'max',
    loggedIn: true,
    folder: '/accounts/max',
    usage,
  };
}

function renderMeter(items: object[]) {
  const api = fakeApi({ 'GET /api/v1/accounts/usage': () => json(200, { items }) });
  const client = createQueryClient({ retry: false });
  const view = render(
    <QueryClientProvider client={client}>
      <UsageMeter />
    </QueryClientProvider>,
  );
  return { ...view, api, client };
}

describe('UsageMeter', () => {
  it('shows the five hour window of the first account', async () => {
    renderMeter([
      status({
        available: true,
        reason: null,
        plan: 'max',
        fiveHour: { percent: 42, resetsAt: null },
        sevenDay: null,
        fetchedAt: 1,
      }),
    ]);
    expect(await screen.findByRole('progressbar')).toHaveValue(42);
    expect(screen.getByText('42% of 5 h')).toBeVisible();
  });

  it('says so when there are no numbers', async () => {
    renderMeter([
      status({
        available: false,
        reason: 'no-login',
        plan: null,
        fiveHour: null,
        sevenDay: null,
        fetchedAt: null,
      }),
    ]);
    expect(await screen.findByText('Claude usage unavailable')).toBeVisible();
  });

  it('shows nothing without an account', async () => {
    const { container, client } = renderMeter([]);
    await waitFor(() => {
      expect(client.getQueryState(usageQuery().queryKey)?.status).toBe('success');
    });
    expect(container).toBeEmptyDOMElement();
  });
});
