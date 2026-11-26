import { useQuery, useQueryClient, type UseQueryResult } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import { dispatcherConfigQuery, dispatcherKeys } from './queries';
import { nextEdgeDelay } from './statusEdge';
import {
  dispatcherStatusQuery,
  usePauseDispatcher,
  useResumeDispatcher,
  useRunDispatcherPass,
  useSetDispatcherEnabled,
  type DispatcherStatus,
} from './statusQueries';
import './dispatcher.css';

export function stateLabel(status: DispatcherStatus): string {
  switch (status.state) {
    case 'off':
      return 'Off';
    case 'idle':
      return 'Idle';
    case 'running':
      return `Running ${String(status.active)}/${String(status.wip)}`;
    case 'paused':
      return 'Paused';
    case 'outside-window':
      return 'Outside window';
  }
}

// The state depends on the clock: refetch once the pause or the window edge has passed.
const EDGE_MARGIN_MS = 1000;
const MAX_TIMER_MS = 2_147_483_647;

function useRefetchAtEdge(status: UseQueryResult<DispatcherStatus>) {
  const queryClient = useQueryClient();
  const config = useQuery(dispatcherConfigQuery());
  const pausedUntil = status.data?.pausedUntil;
  const window = config.data?.config.window;
  const from = window?.from;
  const to = window?.to;
  useEffect(() => {
    const edge = from !== undefined && to !== undefined ? { from, to } : null;
    const delay = nextEdgeDelay(new Date(), pausedUntil, edge);
    if (delay === null) return undefined;
    const timer = setTimeout(
      () => {
        void queryClient.invalidateQueries({ queryKey: dispatcherKeys.status() });
      },
      Math.min(delay + EDGE_MARGIN_MS, MAX_TIMER_MS),
    );
    return () => {
      clearTimeout(timer);
    };
  }, [queryClient, pausedUntil, from, to, status.dataUpdatedAt]);
}

export function DispatcherIndicator() {
  const status = useQuery(dispatcherStatusQuery());
  useRefetchAtEdge(status);
  const setEnabled = useSetDispatcherEnabled();
  const pause = usePauseDispatcher();
  const resume = useResumeDispatcher();
  const runPass = useRunDispatcherPass();
  const [open, setOpen] = useState(false);
  const data = status.data;
  if (!data) return null;

  const error = [setEnabled, pause, resume, runPass].find((action) => action.isError)?.error;
  const paused = data.state === 'paused';

  function act(run: () => Promise<unknown>) {
    setOpen(false);
    run().catch(() => undefined);
  }

  return (
    <div className="dispatcher-indicator">
      <button
        type="button"
        className={`btn dispatcher-state is-${data.state}`}
        title={data.message}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => {
          setOpen(!open);
        }}
      >
        Dispatcher: {stateLabel(data)}
      </button>
      {open && (
        <div className="dispatcher-menu" role="menu">
          <button
            type="button"
            role="menuitem"
            className="btn"
            onClick={() => {
              act(() => setEnabled.mutateAsync(!data.enabled));
            }}
          >
            {data.enabled ? 'Disable' : 'Enable'}
          </button>
          {paused ? (
            <button
              type="button"
              role="menuitem"
              className="btn"
              onClick={() => {
                act(() => resume.mutateAsync());
              }}
            >
              Resume
            </button>
          ) : (
            <button
              type="button"
              role="menuitem"
              className="btn"
              onClick={() => {
                act(() => pause.mutateAsync());
              }}
            >
              Pause 1 h
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            className="btn"
            onClick={() => {
              act(() => runPass.mutateAsync());
            }}
          >
            Run pass now
          </button>
        </div>
      )}
      <FormError message={problemMessage(error, {}, 'The dispatcher request failed.')} />
    </div>
  );
}
