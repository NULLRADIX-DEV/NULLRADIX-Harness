import { useQuery } from '@tanstack/react-query';
import { problemMessage } from '../../shared/api/problemMessage';
import { claudeThemeQuery, useSetClaudeTheme, type ClaudeTheme } from '../../shared/query/themes';
import { FormError } from '../../shared/ui/FormError';
import { Panel } from '../../shared/ui/Panel';

const MESSAGES = {};

export function claudeThemeHint(status: ClaudeTheme): string {
  if (!status.enabled) return 'Off: Claude keeps its own look.';
  if (status.state === 'none') return 'The chosen theme brings no Claude theme.';
  const name = status.themeName ?? status.themeId ?? 'The theme';
  if (status.state === 'active') return `Active in Claude: ${name}`;
  return `${name} could not be put into every Claude folder.`;
}

// Hooks of a theme are code that runs inside Claude, so only this switch applies them.
export function ClaudeThemeSection() {
  const status = useQuery(claudeThemeQuery());
  const set = useSetClaudeTheme();
  const data = status.data;
  const warn = data?.state === 'foreign' || data?.state === 'error';
  return (
    <Panel as="section" className="panel page-section" aria-labelledby="claude-theme-heading">
      <h2 id="claude-theme-heading">Claude theme</h2>
      <p className="muted">
        A theme can bring hooks and styles for Claude. They run as code inside Claude, in the
        default Claude folder and in every account folder, and only while this switch is on.
      </p>
      {status.isError && <FormError message="Could not load the Claude theme state." />}
      <label className="theme-check">
        <input
          type="checkbox"
          role="switch"
          disabled={!data || set.isPending}
          checked={data?.enabled ?? false}
          onChange={(event) => {
            set.mutate(event.target.checked);
          }}
        />
        <span>Let Claude use the chosen theme</span>
      </label>
      {data && (
        <p className={warn ? 'claude-theme-warn' : 'field-hint muted'}>{claudeThemeHint(data)}</p>
      )}
      {data && data.enabled && data.targets.length > 0 && (
        <ul className="claude-theme-targets" aria-label="Claude folders">
          {data.targets.map((target) => (
            <li key={target.path}>
              <code>{target.path}</code> <span className="muted">{target.state}</span>
              {target.message && <div className="claude-theme-warn">{target.message}</div>}
            </li>
          ))}
        </ul>
      )}
      <FormError message={problemMessage(set.error, MESSAGES, 'Could not change the switch.')} />
    </Panel>
  );
}
