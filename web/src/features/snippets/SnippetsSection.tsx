import { useQuery } from '@tanstack/react-query';
import { useState, type SubmitEvent } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { snippetId } from '../../shared/lib/snippets';
import { FormError } from '../../shared/ui/FormError';
import {
  snippetsQuery,
  useCreateSnippet,
  useDeleteSnippet,
  useUpdateSnippet,
  type Snippet,
} from './queries';
import './snippets.css';

const NAME_MAX = 100;

const MESSAGES = {
  'version-conflict': 'This snippet changed in the meantime. The list is up to date now.',
  'snippet-missing': 'This snippet no longer exists.',
  'snippet-exists': 'A snippet with this id exists already.',
  'snippet-id-invalid': 'The name does not give a usable id.',
  'snippet-name-invalid': `Give the snippet a name of at most ${String(NAME_MAX)} characters.`,
  'snippet-too-large': 'The style text is larger than 64 KiB.',
};

function CreateForm({ taken }: { taken: readonly string[] }) {
  const [name, setName] = useState('');
  const [css, setCss] = useState('');
  const create = useCreateSnippet();

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    create.mutate(
      { id: snippetId(name, taken), name: name.trim(), css },
      {
        onSuccess: () => {
          setName('');
          setCss('');
        },
      },
    );
  }

  return (
    <form className="snippet-create" aria-label="New snippet" onSubmit={submit}>
      <label className="field">
        <span>Name</span>
        <input
          value={name}
          maxLength={NAME_MAX}
          autoComplete="off"
          onChange={(event) => {
            setName(event.target.value);
          }}
        />
      </label>
      <label className="field">
        <span>Style</span>
        <textarea
          className="snippet-content"
          value={css}
          spellCheck={false}
          placeholder="a { color: red; }"
          onChange={(event) => {
            setCss(event.target.value);
          }}
        />
      </label>
      <button
        type="submit"
        className="btn primary"
        disabled={create.isPending || name.trim() === ''}
      >
        Add snippet
      </button>
      <FormError message={problemMessage(create.error, MESSAGES, 'Could not add the snippet.')} />
    </form>
  );
}

function EditForm({ item, onDone }: { item: Snippet; onDone: () => void }) {
  const [name, setName] = useState(item.name);
  const [css, setCss] = useState(item.css);
  const update = useUpdateSnippet();

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    update.mutate(
      {
        id: item.id,
        version: item.version,
        ...(name.trim() !== item.name && { name: name.trim() }),
        ...(css !== item.css && { css }),
      },
      { onSuccess: onDone },
    );
  }

  return (
    <form aria-label={`Edit ${item.name}`} onSubmit={submit}>
      <label className="field">
        <span>Name of {item.name}</span>
        <input
          value={name}
          maxLength={NAME_MAX}
          autoComplete="off"
          onChange={(event) => {
            setName(event.target.value);
          }}
        />
      </label>
      <textarea
        className="snippet-content"
        aria-label={`Style of ${item.name}`}
        value={css}
        spellCheck={false}
        onChange={(event) => {
          setCss(event.target.value);
        }}
      />
      <div className="form-row">
        <button
          type="submit"
          className="btn primary"
          disabled={
            update.isPending ||
            name.trim() === '' ||
            (name.trim() === item.name && css === item.css)
          }
        >
          Save
        </button>
        <button type="button" className="btn" onClick={onDone}>
          Cancel
        </button>
      </div>
      <FormError message={problemMessage(update.error, MESSAGES, 'Could not save the snippet.')} />
    </form>
  );
}

function Row({ item }: { item: Snippet }) {
  const [mode, setMode] = useState<'view' | 'edit' | 'delete'>('view');
  const update = useUpdateSnippet();
  const remove = useDeleteSnippet();
  const back = () => {
    setMode('view');
  };

  return (
    <li className="snippet-row">
      <div className="snippet-line">
        <div className="snippet-title">
          <strong>{item.name}</strong>{' '}
          <span className="muted mono">
            {item.id} - {item.enabled ? 'On' : 'Off'}
          </span>
        </div>
        {mode === 'view' && (
          <span className="project-actions">
            <button
              type="button"
              className="btn"
              onClick={() => {
                setMode('edit');
              }}
            >
              Edit {item.name}
            </button>{' '}
            <button
              type="button"
              className="btn"
              disabled={update.isPending}
              onClick={() => {
                update.mutate({ id: item.id, version: item.version, enabled: !item.enabled });
              }}
            >
              {item.enabled ? `Turn off ${item.name}` : `Turn on ${item.name}`}
            </button>{' '}
            <button
              type="button"
              className="btn"
              onClick={() => {
                setMode('delete');
              }}
            >
              Delete {item.name}
            </button>
          </span>
        )}
      </div>
      {mode === 'edit' && <EditForm item={item} onDone={back} />}
      {mode === 'delete' && (
        <div role="group" aria-label={`Delete ${item.name}`}>
          <p>
            Delete <strong>{item.name}</strong>? Its style no longer applies.
          </p>
          <button
            type="button"
            className="btn danger"
            disabled={remove.isPending}
            onClick={() => {
              remove.mutate({ id: item.id, version: item.version }, { onSuccess: back });
            }}
          >
            Delete
          </button>{' '}
          <button type="button" className="btn" onClick={back}>
            Keep
          </button>
        </div>
      )}
      <FormError
        message={problemMessage(
          update.error ?? remove.error,
          MESSAGES,
          'Could not change the snippet.',
        )}
      />
    </li>
  );
}

export function SnippetsSection() {
  const snippets = useQuery(snippetsQuery());
  const items = snippets.data ?? [];
  return (
    <section className="panel page-section" aria-labelledby="snippets-heading">
      <h2 id="snippets-heading">CSS snippets</h2>
      <p className="muted">
        Your own styles on top of the theme. Snippets that are on apply in the order of this list;
        links to other sites and imports are refused.
      </p>
      {snippets.isError && <FormError message="Could not load the snippets." />}
      {snippets.data && items.length === 0 && <p className="muted">No snippets yet.</p>}
      <ul className="snippet-list" aria-label="Snippets">
        {items.map((item) => (
          <Row key={item.id} item={item} />
        ))}
      </ul>
      <CreateForm taken={items.map((item) => item.id)} />
    </section>
  );
}
