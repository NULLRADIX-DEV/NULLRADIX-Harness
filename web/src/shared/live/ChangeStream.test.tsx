import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { FakeEventSource } from '../../test/eventSource';
import { ChangeStream } from './ChangeStream';

const registry = { project: [['projects']] };

function renderStream(enabled: boolean, queryClient = new QueryClient()) {
  const ui = (on: boolean) => (
    <QueryClientProvider client={queryClient}>
      <ChangeStream enabled={on} registry={registry}>
        <p>content</p>
      </ChangeStream>
    </QueryClientProvider>
  );
  const view = render(ui(enabled));
  return {
    ...view,
    setEnabled: (on: boolean) => {
      view.rerender(ui(on));
    },
  };
}

describe('ChangeStream', () => {
  it('stays closed while disabled', () => {
    const { getByText } = renderStream(false);
    expect(getByText('content')).toBeInTheDocument();
    expect(FakeEventSource.instances).toHaveLength(0);
  });

  it('opens when enabled and closes when disabled or unmounted', () => {
    const { setEnabled, unmount } = renderStream(true);
    const first = FakeEventSource.latest();
    expect(first.readyState).toBe(FakeEventSource.CONNECTING);

    setEnabled(false);
    expect(first.readyState).toBe(FakeEventSource.CLOSED);

    setEnabled(true);
    const second = FakeEventSource.latest();
    expect(second).not.toBe(first);
    unmount();
    expect(second.readyState).toBe(FakeEventSource.CLOSED);
  });
});
