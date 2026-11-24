import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render } from '@testing-library/react';
import { StrictMode } from 'react';
import { describe, expect, it, vi } from 'vitest';
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

function openSources() {
  return FakeEventSource.instances.filter((source) => source.readyState !== FakeEventSource.CLOSED);
}

describe('ChangeStream', () => {
  it('keeps exactly one stream open under StrictMode', () => {
    render(
      <StrictMode>
        <QueryClientProvider client={new QueryClient()}>
          <ChangeStream enabled registry={registry}>
            <p>content</p>
          </ChangeStream>
        </QueryClientProvider>
      </StrictMode>,
    );
    expect(openSources()).toHaveLength(1);
  });

  it('passes stream errors to the latest onError', () => {
    const queryClient = new QueryClient();
    const first = vi.fn();
    const second = vi.fn();
    const ui = (onError: () => void) => (
      <QueryClientProvider client={queryClient}>
        <ChangeStream enabled registry={registry} onError={onError}>
          <p>content</p>
        </ChangeStream>
      </QueryClientProvider>
    );
    const { rerender } = render(ui(first));
    rerender(ui(second));
    FakeEventSource.latest().fail();
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledOnce();
    expect(FakeEventSource.instances).toHaveLength(1);
  });

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
