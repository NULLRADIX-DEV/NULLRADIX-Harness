import { useQuery } from '@tanstack/react-query';
import { useState, type SubmitEvent } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import {
  catalogSourcesQuery,
  catalogThemesQuery,
  useAddCatalogSource,
  useInstallFromCatalog,
  useRemoveCatalogSource,
  type CatalogSourceStatus,
  type CatalogTheme,
} from '../../shared/query/themeCatalog';
import { FormError } from '../../shared/ui/FormError';
import { ThemeConfirmDialog, confirmationOf } from './ThemeConfirmDialog';
import './themes.css';

const ADD_MESSAGES = {
  'catalog-source-exists': 'This address is already a source.',
  'catalog-source-limit': 'There are too many sources. Remove one first.',
};

const REMOVE_MESSAGES = {
  'version-conflict': 'The sources changed in the meantime. The list is up to date now.',
  'catalog-source-not-found': 'This source is already gone.',
};

const INSTALL_MESSAGES = {
  'catalog-checksum-mismatch': 'The download does not match the catalog and was not installed.',
  'catalog-theme-incompatible': 'This theme does not fit this version of NULLRADIX.',
  'catalog-theme-installed-elsewhere':
    'This theme is installed from another source or by upload. Replace it to continue.',
  'catalog-theme-not-found': 'The source does not offer this theme any more.',
  'catalog-timeout': 'The source did not answer in time.',
  'catalog-unreachable': 'The source cannot be reached.',
  'catalog-http-error': 'The source answered with an error.',
  'catalog-too-large': 'The download is larger than allowed.',
  'catalog-address-refused': 'The download address is not allowed.',
};

function stateText(theme: CatalogTheme): string {
  if (theme.state === 'installed') return `Installed v${theme.installedVersion ?? ''}`;
  if (theme.state === 'update') return `Update from v${theme.installedVersion ?? ''}`;
  if (theme.state === 'other-source') {
    return `Installed v${theme.installedVersion ?? ''} from another source`;
  }
  return 'Not installed';
}

function ThemeRow({ theme }: { theme: CatalogTheme }) {
  const install = useInstallFromCatalog();
  const [confirming, setConfirming] = useState(false);
  const facts = confirmationOf(install.error);
  const asked = install.variables;
  const elsewhere = theme.state === 'other-source';
  const label = theme.state === 'update' ? 'Update' : elsewhere ? 'Replace' : 'Install';
  return (
    <li className="theme-row">
      <div className="theme-row-text">
        <strong>{theme.name}</strong>
        <div className="muted">
          v{theme.version} - {stateText(theme)}
        </div>
        {theme.description && <div className="muted">{theme.description}</div>}
        {!theme.compatible && <div className="muted">Needs SDK {theme.sdkVersion}.</div>}
      </div>
      {theme.compatible && theme.state !== 'installed' && (
        <button
          type="button"
          className="btn"
          disabled={install.isPending}
          aria-label={`${label} ${theme.name}`}
          onClick={() => {
            if (elsewhere) setConfirming(true);
            else install.mutate({ sourceId: theme.sourceId, id: theme.id });
          }}
        >
          {install.isPending ? 'Working...' : label}
        </button>
      )}
      {confirming && elsewhere && (
        <div
          className="project-confirm"
          role="group"
          aria-label={`Replace ${theme.name}`}
          onKeyDown={(event) => {
            if (event.key === 'Escape') setConfirming(false);
          }}
        >
          <p>
            <strong>{theme.name}</strong> is installed from another source or by upload. Replace it
            with this one?
          </p>
          <div className="form-row">
            <button
              type="button"
              className="btn danger"
              disabled={install.isPending}
              onClick={() => {
                install.mutate(
                  { sourceId: theme.sourceId, id: theme.id, replace: true },
                  {
                    onSettled: () => {
                      setConfirming(false);
                    },
                  },
                );
              }}
            >
              Replace
            </button>
            <button
              type="button"
              className="btn"
              onClick={() => {
                setConfirming(false);
              }}
            >
              Cancel
            </button>
          </div>
        </div>
      )}
      <FormError
        message={
          facts
            ? null
            : problemMessage(install.error, INSTALL_MESSAGES, 'Could not install the theme.')
        }
      />
      {facts && asked && (
        <ThemeConfirmDialog
          name={theme.name}
          facts={facts}
          busy={install.isPending}
          onConfirm={() => {
            install.mutate({ ...asked, confirmedSha256: facts.sha256 ?? undefined });
          }}
          onCancel={install.reset}
        />
      )}
    </li>
  );
}

function Problems({ sources }: { sources: CatalogSourceStatus[] }) {
  return (
    <>
      {sources.map((status) => (
        <div key={status.source.id}>
          {status.problem && (
            <FormError message={`${status.source.url}: ${status.problem.message}.`} />
          )}
          {status.skipped.length > 0 && (
            <details className="catalog-skipped muted">
              <summary>
                {status.skipped.length} entries of {status.source.url} were skipped
              </summary>
              <ul>
                {status.skipped.map((skipped) => (
                  <li key={skipped.index}>
                    #{skipped.index}
                    {skipped.id && ` (${skipped.id})`}: {skipped.reason}
                  </li>
                ))}
              </ul>
            </details>
          )}
        </div>
      ))}
    </>
  );
}

export function CatalogSection() {
  const sources = useQuery(catalogSourcesQuery());
  const items = sources.data?.items ?? [];
  const themes = useQuery(catalogThemesQuery(items.length > 0));
  const add = useAddCatalogSource();
  const remove = useRemoveCatalogSource();
  const [url, setUrl] = useState('');

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate(url, {
      onSuccess: () => {
        setUrl('');
      },
    });
  }

  return (
    <section className="panel page-section" aria-labelledby="catalog-heading">
      <h2 id="catalog-heading">Theme catalog</h2>
      {sources.isError && <FormError message="Could not load the catalog sources." />}
      {sources.data && items.length === 0 && (
        <p className="muted">
          No source yet. A source is the https address of an index file that lists themes.
        </p>
      )}
      <ul className="theme-list" aria-label="Catalog sources">
        {items.map((source) => (
          <li key={source.id} className="theme-row">
            <div className="theme-row-text">{source.url}</div>
            <button
              type="button"
              className="btn"
              disabled={remove.isPending}
              aria-label={`Remove source ${source.url}`}
              onClick={() => {
                remove.mutate({ id: source.id, version: sources.data?.version ?? 0 });
              }}
            >
              Remove
            </button>
          </li>
        ))}
      </ul>
      <FormError
        message={problemMessage(remove.error, REMOVE_MESSAGES, 'Could not remove the source.')}
      />
      <form className="catalog-add" onSubmit={submit}>
        <label className="field">
          <span>Add a source</span>
          <input
            type="url"
            value={url}
            placeholder="https://example.com/themes.json"
            onChange={(event) => {
              setUrl(event.target.value);
            }}
          />
        </label>
        <button type="submit" className="btn" disabled={add.isPending || url.trim() === ''}>
          Add source
        </button>
      </form>
      <FormError message={problemMessage(add.error, ADD_MESSAGES, 'Could not add the source.')} />
      {items.length > 0 && (
        <>
          <h3>Available themes</h3>
          <button
            type="button"
            className="btn"
            disabled={themes.isFetching}
            onClick={() => {
              void themes.refetch();
            }}
          >
            {themes.isFetching ? 'Loading...' : 'Refresh'}
          </button>
          {themes.isError && <FormError message="Could not load the catalog." />}
          {themes.data && (
            <>
              <Problems sources={themes.data.sources} />
              {themes.data.items.length === 0 && <p className="muted">No themes offered.</p>}
              <ul className="theme-list" aria-label="Available themes">
                {themes.data.items.map((theme) => (
                  <ThemeRow key={`${theme.sourceId}/${theme.id}`} theme={theme} />
                ))}
              </ul>
            </>
          )}
        </>
      )}
    </section>
  );
}
