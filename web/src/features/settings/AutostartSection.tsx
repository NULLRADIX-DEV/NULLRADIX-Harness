import { useQuery } from '@tanstack/react-query';
import { problemMessage } from '../../shared/api/problemMessage';
import { autostartQuery, useSetAutostart, type Autostart } from '../../shared/query/autostart';
import { FormError } from '../../shared/ui/FormError';

const MESSAGES = {
  'autostart-failed': 'The system refused the login entry; the daemon log says why.',
};

export function autostartHint(status: Autostart): string {
  if (!status.available) return 'This host cannot start NULLRADIX at login.';
  return status.enabled
    ? 'NULLRADIX starts when you log in.'
    : 'Off: NULLRADIX starts when you open it.';
}

export function AutostartSection() {
  const status = useQuery(autostartQuery());
  const set = useSetAutostart();
  const data = status.data;
  return (
    <section className="panel page-section" aria-labelledby="autostart-heading">
      <h2 id="autostart-heading">Start at login</h2>
      {status.isError && <FormError message="Could not load the login setting." />}
      <label className="theme-check">
        <input
          type="checkbox"
          role="switch"
          disabled={!data?.available || set.isPending}
          checked={data?.enabled ?? false}
          onChange={(event) => {
            set.mutate(event.target.checked);
          }}
        />
        <span>Start NULLRADIX when I log in</span>
      </label>
      {data && <p className="field-hint muted">{autostartHint(data)}</p>}
      <FormError message={problemMessage(set.error, MESSAGES, 'Could not change the setting.')} />
    </section>
  );
}
