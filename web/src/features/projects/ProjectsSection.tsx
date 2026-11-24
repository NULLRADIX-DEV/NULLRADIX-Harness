import { useQuery } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState, type SubmitEvent } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import {
  projectsQuery,
  useCreateProject,
  useDeleteProject,
  useRenameProject,
  type Project,
} from './queries';

export const KEY_MAX_LENGTH = 6;
export const NAME_MAX_LENGTH = 200;

export const CONFLICT_MESSAGE = 'This project changed in the meantime. The list is up to date now.';
export const GONE_MESSAGE = 'This project no longer exists.';

const MESSAGES = {
  'version-conflict': CONFLICT_MESSAGE,
  'project-not-found': GONE_MESSAGE,
};

function CreateProjectForm() {
  const [key, setKey] = useState('');
  const [name, setName] = useState('');
  const create = useCreateProject();
  const hintId = useId();

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    create.mutate(
      { key, name },
      {
        onSuccess: () => {
          setKey('');
          setName('');
        },
      },
    );
  }

  return (
    <form className="project-create" aria-label="New project" onSubmit={submit}>
      <div className="form-row">
        <label className="field project-key-field">
          <span>Key</span>
          <input
            className="mono"
            value={key}
            maxLength={KEY_MAX_LENGTH}
            autoComplete="off"
            spellCheck={false}
            aria-describedby={hintId}
            onChange={(event) => {
              setKey(event.target.value.toUpperCase());
            }}
          />
        </label>
        <label className="field project-name-field">
          <span>Name</span>
          <input
            value={name}
            maxLength={NAME_MAX_LENGTH}
            autoComplete="off"
            onChange={(event) => {
              setName(event.target.value);
            }}
          />
        </label>
        <button
          type="submit"
          className="btn primary"
          disabled={create.isPending || key === '' || name.trim() === ''}
        >
          Create project
        </button>
      </div>
      <p id={hintId} className="field-hint muted">
        2 to 6 characters, a letter first, then letters or digits.
      </p>
      <FormError
        message={problemMessage(create.error, MESSAGES, 'Could not create the project.')}
      />
    </form>
  );
}

function RenameForm({ project, onDone }: { project: Project; onDone: () => void }) {
  const [name, setName] = useState(project.name);
  const rename = useRenameProject();
  const trimmed = name.trim();
  const unchanged = trimmed === '' || trimmed === project.name;

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    rename.mutate({ id: project.id, name, version: project.version }, { onSuccess: onDone });
  }

  return (
    <form className="project-edit" aria-label={`Rename ${project.key}`} onSubmit={submit}>
      <div className="form-row">
        <label className="field project-name-field">
          <span className="visually-hidden">Name</span>
          <input
            value={name}
            maxLength={NAME_MAX_LENGTH}
            autoComplete="off"
            autoFocus
            onChange={(event) => {
              setName(event.target.value);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape') onDone();
            }}
          />
        </label>
        <button type="submit" className="btn primary" disabled={rename.isPending || unchanged}>
          Save
        </button>
        <button type="button" className="btn" onClick={onDone}>
          Cancel
        </button>
      </div>
      <FormError
        message={problemMessage(rename.error, MESSAGES, 'Could not rename the project.')}
      />
    </form>
  );
}

function DeleteConfirm({ project, onDone }: { project: Project; onDone: () => void }) {
  const remove = useDeleteProject();
  const cancelButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    cancelButton.current?.focus();
  }, []);

  return (
    <div
      className="project-confirm"
      role="group"
      aria-label={`Delete ${project.key}`}
      onKeyDown={(event) => {
        if (event.key === 'Escape') onDone();
      }}
    >
      <p>
        Delete <strong>{project.name}</strong>? This cannot be undone.
      </p>
      <div className="form-row">
        <button
          type="button"
          className="btn danger"
          disabled={remove.isPending}
          onClick={() => {
            remove.mutate({ id: project.id, version: project.version }, { onSuccess: onDone });
          }}
        >
          Delete
        </button>
        <button ref={cancelButton} type="button" className="btn" onClick={onDone}>
          Cancel
        </button>
      </div>
      <FormError
        message={problemMessage(remove.error, MESSAGES, 'Could not delete the project.')}
      />
    </div>
  );
}

type RowMode = 'view' | 'rename' | 'delete';

function ProjectRow({ project }: { project: Project }) {
  const [mode, setMode] = useState<RowMode>('view');
  const previousMode = useRef<RowMode>('view');
  const renameButton = useRef<HTMLButtonElement>(null);
  const deleteButton = useRef<HTMLButtonElement>(null);
  const back = () => {
    setMode('view');
  };

  useEffect(() => {
    // hand focus back to the button that opened the form
    if (mode === 'view' && previousMode.current === 'rename') renameButton.current?.focus();
    if (mode === 'view' && previousMode.current === 'delete') deleteButton.current?.focus();
    previousMode.current = mode;
  }, [mode]);
  return (
    <li className="project-row">
      <div className="project-line">
        <span className="project-key mono">{project.key}</span>
        <span className="project-name">{project.name}</span>
        {mode === 'view' && (
          <span className="project-actions">
            <button
              ref={renameButton}
              type="button"
              className="btn"
              aria-label={`Rename ${project.key}`}
              onClick={() => {
                setMode('rename');
              }}
            >
              Rename
            </button>
            <button
              ref={deleteButton}
              type="button"
              className="btn"
              aria-label={`Delete ${project.key}`}
              onClick={() => {
                setMode('delete');
              }}
            >
              Delete
            </button>
          </span>
        )}
      </div>
      {mode === 'rename' && <RenameForm project={project} onDone={back} />}
      {mode === 'delete' && <DeleteConfirm project={project} onDone={back} />}
    </li>
  );
}

function ProjectList() {
  const { status, data } = useQuery(projectsQuery());
  if (status === 'pending') return <p className="muted">Loading projects...</p>;
  if (status === 'error') return <p className="form-error">Could not load the projects.</p>;
  if (data.length === 0) return <p className="muted">No projects yet.</p>;
  return (
    <ul className="project-list" aria-label="Projects">
      {data.map((project) => (
        <ProjectRow key={project.id} project={project} />
      ))}
    </ul>
  );
}

export function ProjectsSection() {
  const headingId = useId();
  return (
    <section className="panel page-section" aria-labelledby={headingId}>
      <h2 id={headingId}>Projects</h2>
      <CreateProjectForm />
      <ProjectList />
    </section>
  );
}
