import { useQuery } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import {
  themesQuery,
  useInstallTheme,
  useRemoveTheme,
  useSetThemeCode,
  type Theme,
} from '../../shared/query/themes';
import { FormError } from '../../shared/ui/FormError';
import { ThemeConfirmDialog, confirmationOf } from './ThemeConfirmDialog';
import './themes.css';
import { Button } from '../../shared/ui/Button';
import { Panel } from '../../shared/ui/Panel';

export const PACKAGE_EXTENSION = '.nullradix';

const MESSAGES = {
  'theme-manifest-invalid': 'The manifest of this package is not valid.',
  'theme-package-invalid': 'This is not a valid theme package.',
  'theme-id-reserved': 'The package uses the name of a built-in theme.',
  'version-conflict': 'This theme changed in the meantime. The list is up to date now.',
  'theme-not-found': 'This theme is not installed any more.',
  'theme-has-no-code': 'This theme has no code to run any more.',
};

// Theme code is the one thing here that runs inside the app, so it has its own switch.
function CodeSwitch({ theme }: { theme: Theme }) {
  const set = useSetThemeCode();
  return (
    <div className="theme-code">
      <label className="theme-check">
        <input
          type="checkbox"
          role="switch"
          disabled={set.isPending}
          checked={theme.codeEnabled}
          onChange={(event) => {
            set.mutate({ id: theme.id, enabled: event.target.checked, version: theme.version });
          }}
        />
        <span>Run the code of {theme.name}</span>
      </label>
      <p className="claude-theme-warn">
        Theme code runs in NULLRADIX with full rights. Turn it on only for themes you trust. It
        takes effect after a reload and only while the theme is chosen.
      </p>
      <FormError message={problemMessage(set.error, MESSAGES, 'Could not change the switch.')} />
    </div>
  );
}

function RemoveRow({ theme }: { theme: Theme }) {
  const remove = useRemoveTheme();
  const [asking, setAsking] = useState(false);
  return (
    <li className="theme-row">
      {theme.previewUrl && <img className="theme-preview" src={theme.previewUrl} alt="" />}
      <div className="theme-row-text">
        <strong>{theme.name}</strong>
        <div className="muted">
          {theme.author} - v{theme.packageVersion} - {theme.colorScheme}
        </div>
        {theme.hasCode && <CodeSwitch theme={theme} />}
      </div>
      {asking ? (
        <span role="group" aria-label={`Remove ${theme.name}`}>
          <Button
            type="button"
            variant="danger"
            disabled={remove.isPending}
            onClick={() => {
              remove.mutate({ id: theme.id, version: theme.version });
            }}
          >
            Remove
          </Button>{' '}
          <Button
            type="button"
            onClick={() => {
              setAsking(false);
            }}
          >
            Keep
          </Button>
        </span>
      ) : (
        <Button
          type="button"
          onClick={() => {
            setAsking(true);
          }}
        >
          Remove {theme.name}
        </Button>
      )}
      <FormError message={problemMessage(remove.error, MESSAGES, 'Could not remove the theme.')} />
    </li>
  );
}

export function ThemesSection() {
  const themes = useQuery(themesQuery());
  const install = useInstallTheme();
  const input = useRef<HTMLInputElement>(null);
  const items = themes.data?.items ?? [];
  const facts = confirmationOf(install.error);
  const asked = install.variables;

  return (
    <Panel as="section" className="panel page-section" aria-labelledby="themes-heading">
      <h2 id="themes-heading">Installed themes</h2>
      {themes.isError && <FormError message="Could not load the themes." />}
      {themes.data && items.length === 0 && (
        <p className="muted">
          No theme package is installed. Install one from a {PACKAGE_EXTENSION} file or with
          `nullradix theme install FILE`.
        </p>
      )}
      <ul className="theme-list" aria-label="Installed themes">
        {items.map((theme) => (
          <RemoveRow key={theme.id} theme={theme} />
        ))}
      </ul>
      <div className="theme-install">
        <label className="field">
          <span>Install a theme package</span>
          <input
            ref={input}
            type="file"
            accept={PACKAGE_EXTENSION}
            disabled={install.isPending}
            onChange={(event) => {
              const file = event.target.files?.[0];
              if (!file) return;
              install.mutate(
                { file },
                {
                  onSettled: () => {
                    if (input.current) input.current.value = '';
                  },
                },
              );
            }}
          />
        </label>
        <FormError
          message={
            facts ? null : problemMessage(install.error, MESSAGES, 'Could not install the package.')
          }
        />
        {facts && asked && (
          <ThemeConfirmDialog
            facts={facts}
            busy={install.isPending}
            onConfirm={() => {
              install.mutate({ file: asked.file, confirmed: true });
            }}
            onCancel={install.reset}
          />
        )}
      </div>
    </Panel>
  );
}
