import { STATE_LABELS } from './format';
import type { DisplayState } from './queries';
import './sessions.css';

export function SessionStateBadge({ state }: { state: DisplayState }) {
  return <span className={`session-state session-state-${state}`}>{STATE_LABELS[state]}</span>;
}
