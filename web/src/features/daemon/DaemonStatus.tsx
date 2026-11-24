import { useHealth } from './useHealth';

export function DaemonStatus() {
  const { status, data } = useHealth();
  const label =
    status === 'pending'
      ? 'Checking daemon'
      : status === 'success'
        ? `Daemon running, version ${data.version}`
        : 'Daemon unreachable';
  const state = status === 'pending' ? ' pending' : status === 'error' ? ' off' : '';
  return (
    <span className="shell-status">
      <span className={`live${state}`} role="status">
        <span className="dot" />
        <span className="live-text">{label}</span>
      </span>
    </span>
  );
}
