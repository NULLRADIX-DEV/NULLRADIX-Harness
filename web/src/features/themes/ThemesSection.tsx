import { useQuery } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import {
  themesQuery,
  useInstallTheme,
  useRemoveTheme,
  type Theme,
} from '../../shared/query/themes';
import { FormError } from '../../shared/ui/FormError';
import './themes.css';

export const PACKAGE_EXTENSION = '.nullradix';

const MESSAGES = {
  'theme-manifest-invalid': 'The manifest of this package is not valid.',
  'theme-package-invalid': 'This is not a valid theme package.',
  'theme-id-reserved': 'The package uses the name of a built-in theme.',
  'version-conflict': 'This theme changed in the meantime. The list is up to date now.',
  'theme-not-found': 'This theme is not installed any more.',
};

function RemoveRow({ theme }: { theme: Theme }) {
  const remove = useRemoveTheme();
  const [asking, setAsking] = useState(false);
  return (
    <li className="theme-row">
      {theme.previewUrl && <img className="theme-preview" src={theme.previewUrl} alt="" />}
      <div className="theme-row-text">
        <strong>{theme.name}</strong>
        <div className="muted">
          {theme.author} · v{theme.packageVersion} · {theme.colorScheme}
        </div>
      </div>
      {asking ? (
        <span role="group" aria-label={`Remove ${theme.name}`}>
          <button
            type="button"
            className="btn danger"
            disabled={remove.isPending}
            onClick={() => {
              remove.mutate({ id: theme.id, version: theme.version });
            }}
          >
            Remove
          </button>{' '}
          <button
            type="button"
            className="btn"
            onClick={() => {
              setAsking(false);
            }}
          >
            Keep
          </button>
        </span>
      ) : (
        <button
          type="button"
          className="btn"
          onClick={() => {
            setAsking(true);
          }}
        >
          Remove {theme.name}
        </button>
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

  return (
    <section className="panel page-section" aria-labelledby="themes-heading">
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
              install.mutate(file, {
                onSettled: () => {
                  if (input.current) input.current.value = '';
                },
              });
            }}
          />
        </label>
        <FormError
          message={problemMessage(install.error, MESSAGES, 'Could not install the package.')}
        />
      </div>
    </section>
  );
}
