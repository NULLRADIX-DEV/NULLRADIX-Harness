import { useQuery } from '@tanstack/react-query';
import { useId } from 'react';
import { EXPECTED_API_VERSION } from '../../shared/api/client';
import { healthQuery } from './queries';

type Tone = 'pending' | 'up' | 'incompatible' | 'down';

interface DaemonState {
  tone: Tone;
  summary: string;
  label: string;
}

function useDaemonState(): DaemonState {
  const { status, data } = useQuery(healthQuery());
  if (status === 'pending') {
    return { tone: 'pending', summary: 'checking', label: 'Checking daemon' };
  }
  if (status === 'error') {
    return { tone: 'down', summary: 'unreachable', label: 'Daemon unreachable' };
  }
  if (data.apiVersion !== EXPECTED_API_VERSION) {
    return {
      tone: 'incompatible',
      summary: 'incompatible',
      label: `Incompatible daemon: API version ${String(data.apiVersion)}, expected ${String(EXPECTED_API_VERSION)}`,
    };
  }
  return { tone: 'up', summary: 'running', label: `Daemon running, version ${data.version}` };
}

export function DaemonStatus() {
  const { tone, summary, label } = useDaemonState();
  const labelId = useId();
  return (
    <span className="shell-status">
      <button type="button" className={`live ${tone}`} aria-describedby={labelId}>
        <span className="dot" aria-hidden="true" />
        <span className="visually-hidden">Daemon status: {summary}</span>
      </button>
      <span id={labelId} className="live-text" role="status">
        {label}
      </span>
    </span>
  );
}
