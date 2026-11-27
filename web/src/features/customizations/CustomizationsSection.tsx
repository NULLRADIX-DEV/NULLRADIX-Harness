import { useQuery } from '@tanstack/react-query';
import { useState, type SubmitEvent } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import {
  customizationsQuery,
  useCreateCustomization,
  useDeleteCustomization,
  useUpdateCustomization,
  type Customization,
  type ExtensionPoint,
} from './queries';
import './customizations.css';
import { Input } from '../../shared/ui/Input';
import { Button } from '../../shared/ui/Button';
import { Panel } from '../../shared/ui/Panel';

export const POINT_LABELS: Readonly<Record<ExtensionPoint, string>> = {
  skill: 'Skill',
  'output-style': 'Output style',
};

const STATE_LABELS = {
  active: 'Active',
  fallback: 'Shipped file in charge',
  disabled: 'Disabled',
} as const;

const MESSAGES = {
  'version-conflict': 'This customization changed in the meantime. The list is up to date now.',
  'customization-missing': 'This customization no longer exists.',
  'customization-exists': 'This name is already customized.',
  'customization-name-invalid':
    'Use lowercase letters, digits and single hyphens, at most 64 characters.',
  'customization-content-empty': 'The text is empty.',
  'customization-no-standard': 'Nothing is shipped under this name, so there is nothing to copy.',
};

function CreateForm() {
  const [point, setPoint] = useState<ExtensionPoint>('skill');
  const [name, setName] = useState('');
  const [content, setContent] = useState('');
  const create = useCreateCustomization();

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    create.mutate(
      { point, name, ...(content.trim() !== '' && { content }) },
      {
        onSuccess: () => {
          setName('');
          setContent('');
        },
      },
    );
  }

  return (
    <form className="customization-create" aria-label="New customization" onSubmit={submit}>
      <div className="form-row">
        <label className="field">
          <span>Kind</span>
          <select
            value={point}
            onChange={(event) => {
              setPoint(event.target.value === 'output-style' ? 'output-style' : 'skill');
            }}
          >
            <option value="skill">{POINT_LABELS.skill}</option>
            <option value="output-style">{POINT_LABELS['output-style']}</option>
          </select>
        </label>
        <label className="field">
          <span>Name</span>
          <input
            className="mono"
            value={name}
            maxLength={64}
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => {
              setName(event.target.value);
            }}
          />
        </label>
      </div>
      <label className="field">
        <span>Text</span>
        <Input
          multiline
          className="customization-content"
          value={content}
          placeholder="The whole file with its head. Left empty copies the shipped file."
          onChange={(text) => {
            setContent(text);
          }}
        />
      </label>
      <Button type="submit" variant="primary" disabled={create.isPending || name === ''}>
        Add customization
      </Button>
      <FormError
        message={problemMessage(create.error, MESSAGES, 'Could not add the customization.')}
      />
    </form>
  );
}

function EditForm({ item, onDone }: { item: Customization; onDone: () => void }) {
  const [content, setContent] = useState(item.content);
  const update = useUpdateCustomization();

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    update.mutate(
      { point: item.point, name: item.name, version: item.version, content },
      { onSuccess: onDone },
    );
  }

  return (
    <form aria-label={`Edit ${item.name}`} onSubmit={submit}>
      <Input
        multiline
        className="customization-content"
        aria-label={`Text of ${item.name}`}
        value={content}
        onChange={(text) => {
          setContent(text);
        }}
      />
      <div className="form-row">
        <Button
          type="submit"
          variant="primary"
          disabled={update.isPending || content === item.content}
        >
          Save
        </Button>
        <Button type="button" onClick={onDone}>
          Cancel
        </Button>
      </div>
      <FormError message={problemMessage(update.error, MESSAGES, 'Could not save the text.')} />
    </form>
  );
}

function Row({ item }: { item: Customization }) {
  const [mode, setMode] = useState<'view' | 'edit' | 'delete'>('view');
  const update = useUpdateCustomization();
  const remove = useDeleteCustomization();
  const back = () => {
    setMode('view');
  };

  return (
    <li className="customization-row">
      <div className="customization-line">
        <div className="customization-title">
          <strong className="mono">{item.name}</strong>{' '}
          <span className="muted">
            {POINT_LABELS[item.point]} -{' '}
            {item.kind === 'replace' ? 'replaces the shipped one' : 'new'}
            {' - '}
            {STATE_LABELS[item.state]}
          </span>
        </div>
        {mode === 'view' && (
          <span className="project-actions">
            <Button
              type="button"
              onClick={() => {
                setMode('edit');
              }}
            >
              Edit {item.name}
            </Button>{' '}
            <Button
              type="button"
              disabled={update.isPending}
              onClick={() => {
                update.mutate({
                  point: item.point,
                  name: item.name,
                  version: item.version,
                  enabled: !item.enabled,
                });
              }}
            >
              {item.enabled ? `Disable ${item.name}` : `Enable ${item.name}`}
            </Button>{' '}
            <Button
              type="button"
              onClick={() => {
                setMode('delete');
              }}
            >
              Reset {item.name}
            </Button>
          </span>
        )}
      </div>
      {item.problems.map((problem) => (
        <p key={`${problem.code}:${problem.message}`} className="form-error">
          {problem.message}
        </p>
      ))}
      {item.warnings.map((warning) => (
        <p key={`${warning.code}:${warning.message}`} className="muted">
          {warning.message}
        </p>
      ))}
      {mode === 'edit' && <EditForm item={item} onDone={back} />}
      {mode === 'delete' && (
        <div role="group" aria-label={`Reset ${item.name}`}>
          <p>
            Drop <strong>{item.name}</strong>? The shipped file, if any, is in charge again.
          </p>
          <Button
            type="button"
            variant="danger"
            disabled={remove.isPending}
            onClick={() => {
              remove.mutate(
                { point: item.point, name: item.name, version: item.version },
                { onSuccess: back },
              );
            }}
          >
            Drop
          </Button>{' '}
          <Button type="button" onClick={back}>
            Keep
          </Button>
        </div>
      )}
      <FormError
        message={problemMessage(
          update.error ?? remove.error,
          MESSAGES,
          'Could not change the customization.',
        )}
      />
    </li>
  );
}

export function CustomizationsSection() {
  const customizations = useQuery(customizationsQuery());
  const items = customizations.data ?? [];
  return (
    <Panel as="section" className="panel page-section" aria-labelledby="customizations-heading">
      <h2 id="customizations-heading">Customizations</h2>
      <p className="muted">
        Your own skills and output styles. They replace a shipped one of the same name or come on
        top; a text with problems leaves the shipped file in charge.
      </p>
      {customizations.isError && <FormError message="Could not load the customizations." />}
      {customizations.data && items.length === 0 && (
        <p className="muted">Nothing is customized yet.</p>
      )}
      <ul className="customization-list" aria-label="Customizations">
        {items.map((item) => (
          <Row key={`${item.point}/${item.name}`} item={item} />
        ))}
      </ul>
      <CreateForm />
    </Panel>
  );
}
