import { useQuery } from '@tanstack/react-query';
import { useId, useState, type SubmitEvent } from 'react';
import { ApiProblemError, isProblemCode } from '../../shared/api/errors';
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

function errorMessage(error: unknown, fallback: string): string | null {
  if (!error) return null;
  if (isProblemCode(error, 'version-conflict')) return CONFLICT_MESSAGE;
  if (isProblemCode(error, 'project-not-found')) return GONE_MESSAGE;
  if (error instanceof ApiProblemError && error.status < 500 && error.status !== 401) {
    return error.detail;
  }
  return fallback;
}

function FormError({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <p className="form-error" role="alert">
      {message}
    </p>
  );
}

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
      <FormError message={errorMessage(create.error, 'Could not create the project.')} />
    </form>
  );
}

function RenameForm({ project, onDone }: { project: Project; onDone: () => void }) {
  const [name, setName] = useState(project.name);
  const rename = useRenameProject();

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
        <button type="submit" className="btn primary" disabled={rename.isPending}>
          Save
        </button>
        <button type="button" className="btn" onClick={onDone}>
          Cancel
        </button>
      </div>
      <FormError message={errorMessage(rename.error, 'Could not rename the project.')} />
    </form>
  );
}

function DeleteConfirm({ project, onDone }: { project: Project; onDone: () => void }) {
  const remove = useDeleteProject();
  return (
    <div className="project-confirm" role="group" aria-label={`Delete ${project.key}`}>
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
        <button type="button" className="btn" onClick={onDone}>
          Cancel
        </button>
      </div>
      <FormError message={errorMessage(remove.error, 'Could not delete the project.')} />
    </div>
  );
}

type RowMode = 'view' | 'rename' | 'delete';

function ProjectRow({ project }: { project: Project }) {
  const [mode, setMode] = useState<RowMode>('view');
  const back = () => {
    setMode('view');
  };
  return (
    <li className="project-row">
      <div className="project-line">
        <span className="project-key mono">{project.key}</span>
        <span className="project-name">{project.name}</span>
        {mode === 'view' && (
          <span className="project-actions">
            <button
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
