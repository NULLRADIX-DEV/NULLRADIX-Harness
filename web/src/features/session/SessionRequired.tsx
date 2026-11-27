import { BrandMark } from '../../shared/ui/BrandMark';
import { DaemonStatus } from '../daemon/DaemonStatus';
import { Panel } from '../../shared/ui/Panel';

export function SessionRequired() {
  return (
    <main className="gate">
      <Panel className="gate-card panel">
        <div className="dock-row">
          <DaemonStatus />
          <span className="shell-brand">
            <BrandMark />
            NULLRADIX
          </span>
        </div>
        <h1>Open NULLRADIX from your terminal</h1>
        <p className="muted">
          This browser has no session with the daemon. Run this command on the machine where the
          daemon runs; it opens a signed-in window.
        </p>
        <pre className="gate-command">
          <code>nullradix open</code>
        </pre>
        <p className="muted gate-hint">This page checks again every few seconds.</p>
      </Panel>
    </main>
  );
}
