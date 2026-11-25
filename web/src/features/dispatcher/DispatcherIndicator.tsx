import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
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

export function DispatcherIndicator() {
  const status = useQuery(dispatcherStatusQuery());
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
