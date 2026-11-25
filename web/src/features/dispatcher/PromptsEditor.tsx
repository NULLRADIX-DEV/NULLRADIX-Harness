import { useQuery } from '@tanstack/react-query';
import { useLayoutEffect, useRef, useState } from 'react';
import { ApiProblemError } from '../../shared/api/errors';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import { TextSetting } from './controls';
import {
  promptsQuery,
  usePreviewPrompt,
  useResetPrompt,
  useSavePrompt,
  type PromptDto,
  type PromptKind,
} from './queries';

export const PROMPT_CONFLICT_MESSAGE =
  'The dispatcher configuration changed in the meantime. Reload to see the new version.';

const MESSAGES = { 'version-conflict': PROMPT_CONFLICT_MESSAGE };

const effective = (prompt: PromptDto): string => prompt.template ?? prompt.default;

// The daemon lists every refused part of a template; each one gets its own line.
function templateProblems(error: unknown): string[] {
  if (!(error instanceof ApiProblemError) || error.code !== 'dispatcher-prompt-invalid') return [];
  return error.fields.map((issue) => issue.message);
}

function Preview({ prompt, draft }: { prompt: PromptDto; draft: string }) {
  const [key, setKey] = useState('');
  const preview = usePreviewPrompt();
  const research = prompt.kind === 'research';
  const problems = templateProblems(preview.error);
  return (
    <div className="prompt-preview">
      <h4>Preview</h4>
      <TextSetting
        label={research ? 'Question key' : 'Ticket key'}
        hint="Leave empty to use sample values. The text above is previewed, saved or not."
        placeholder={research ? 'ARC-Q0001' : 'ARC-0001'}
        short
        value={key}
        onChange={setKey}
      />
      <div>
        <button
          type="button"
          className="btn"
          disabled={preview.isPending}
          onClick={() => {
            preview.mutate({ kind: prompt.kind, key, template: draft });
          }}
        >
          Preview
        </button>
      </div>
      {problems.length > 0 && (
        <ul className="dispatcher-row-errors" aria-label="Preview problems">
          {problems.map((message) => (
            <li key={message} className="form-error" role="alert">
              {message}
            </li>
          ))}
        </ul>
      )}
      {problems.length === 0 && (
        <FormError message={problemMessage(preview.error, {}, 'Could not render the preview.')} />
      )}
      {preview.data !== undefined && (
        <>
          <pre className="prompt-text" aria-label="Rendered instruction">
            {preview.data.prompt}
          </pre>
          {preview.data.sample && (
            <p className="field-hint muted">
              These are sample values; give a key to use a real one.
            </p>
          )}
          {preview.data.standIns.length > 0 && (
            <p className="field-hint muted">
              Stand-in text for {preview.data.standIns.join(', ')}: there is no data for it yet.
            </p>
          )}
        </>
      )}
    </div>
  );
}

interface Base {
  prompt: PromptDto;
  version: number;
}

function PromptEditor({ prompt, version }: Base) {
  const [base, setBase] = useState<Base>({ prompt, version });
  const [draft, setDraft] = useState(effective(prompt));
  const [saved, setSaved] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);
  const cursor = useRef<number | null>(null);
  const save = useSavePrompt();
  const reset = useResetPrompt();
  const dirty = draft !== effective(base.prompt);

  // a version from elsewhere replaces the text only while nothing is being edited
  if ((prompt !== base.prompt || version !== base.version) && !dirty) {
    setBase({ prompt, version });
    setDraft(effective(prompt));
  }
  const behind = version !== base.version;
  const failure = save.error ?? reset.error;
  const problems = templateProblems(failure);

  useLayoutEffect(() => {
    if (cursor.current === null || area.current === null) return;
    area.current.focus();
    area.current.setSelectionRange(cursor.current, cursor.current);
    cursor.current = null;
  }, [draft]);

  function edit(text: string) {
    setDraft(text);
    setSaved(false);
    save.reset();
    reset.reset();
  }

  function insert(name: string) {
    const field = area.current;
    const text = `{{${name}}}`;
    const start = field?.selectionStart ?? draft.length;
    const end = field?.selectionEnd ?? draft.length;
    cursor.current = start + text.length;
    edit(draft.slice(0, start) + text + draft.slice(end));
  }

  function adopt(next: Base) {
    setBase(next);
    setDraft(effective(next.prompt));
    setSaved(true);
  }

  function reload() {
    setBase({ prompt, version });
    setDraft(effective(prompt));
    setSaved(false);
    save.reset();
    reset.reset();
  }

  function submit() {
    const done = {
      onSuccess: (next: { prompt: PromptDto; version: number }) => {
        adopt(next);
      },
    };
    if (draft === base.prompt.default && base.prompt.template !== null) {
      reset.mutate({ kind: prompt.kind, version: base.version }, done);
    } else {
      save.mutate({ kind: prompt.kind, template: draft, version: base.version }, done);
    }
  }

  function dropOwn() {
    reset.mutate(
      { kind: prompt.kind, version: base.version },
      {
        onSuccess: (next) => {
          adopt(next);
        },
      },
    );
  }

  const label = `${base.prompt.label} template`;
  return (
    <div className="prompt-editor">
      {behind && (
        <p className="form-error" role="alert">
          {PROMPT_CONFLICT_MESSAGE}{' '}
          <button type="button" className="btn" onClick={reload}>
            Reload
          </button>
        </p>
      )}
      <div className="setting wide">
        <label htmlFor={`prompt-${prompt.kind}`}>{label}</label>
        <textarea
          id={`prompt-${prompt.kind}`}
          ref={area}
          className="mono prompt-area"
          rows={16}
          spellCheck={false}
          value={draft}
          onChange={(event) => {
            edit(event.target.value);
          }}
        />
        <p className="field-hint muted" role="status">
          {base.prompt.template === null ? 'Using the default template.' : 'Using an own template.'}
          {dirty && ' Unsaved changes.'}
          {saved && !dirty && ' Saved.'}
        </p>
      </div>
      {problems.length > 0 && (
        <ul className="dispatcher-row-errors" aria-label="Template problems">
          {problems.map((message) => (
            <li key={message} className="form-error" role="alert">
              {message}
            </li>
          ))}
        </ul>
      )}
      {problems.length === 0 && !behind && (
        <FormError message={problemMessage(failure, MESSAGES, 'Could not save the template.')} />
      )}
      <div className="dispatcher-footer">
        <button
          type="button"
          className="btn primary"
          disabled={!dirty || save.isPending || reset.isPending}
          onClick={submit}
        >
          Save template
        </button>
        <button
          type="button"
          className="btn"
          disabled={base.prompt.template === null || save.isPending || reset.isPending}
          onClick={dropOwn}
        >
          Reset to default
        </button>
      </div>
      <div className="prompt-help">
        <div>
          <h4>Placeholders</h4>
          <ul className="prompt-list" aria-label="Placeholders">
            {base.prompt.placeholders.map((placeholder) => (
              <li key={placeholder.name}>
                <button
                  type="button"
                  className="btn mono"
                  aria-label={`Insert ${placeholder.name}`}
                  onClick={() => {
                    insert(placeholder.name);
                  }}
                >
                  {`{{${placeholder.name}}}`}
                </button>
                <span className="muted">{placeholder.description}</span>
              </li>
            ))}
          </ul>
        </div>
        <div>
          <h4>Required parts</h4>
          <p className="field-hint muted">A template without these is refused.</p>
          <ul className="prompt-list" aria-label="Required parts">
            {base.prompt.required.map((part) => (
              <li key={part} className="mono">
                {part}
              </li>
            ))}
          </ul>
        </div>
      </div>
      <Preview prompt={base.prompt} draft={draft} />
    </div>
  );
}

export function PromptsEditor() {
  const prompts = useQuery(promptsQuery());
  const [kind, setKind] = useState<PromptKind>('work');
  if (prompts.status === 'pending') return <p className="muted">Loading the prompts...</p>;
  if (prompts.status === 'error') return <p className="form-error">Could not load the prompts.</p>;
  const items = prompts.data.items;
  const current = items.find((item) => item.kind === kind) ?? items[0];
  return (
    <section className="dispatcher-group" aria-label="Prompts">
      <h3>Prompts</h3>
      <p className="field-hint muted">
        The instruction Claude gets for each kind of run. Placeholders such as {'{{ticketId}}'} are
        filled in per run. Own templates replace the default.
      </p>
      <div className="prompt-kinds" role="group" aria-label="Kind of run">
        {items.map((item) => (
          <button
            key={item.kind}
            type="button"
            className="btn"
            aria-pressed={item.kind === current?.kind}
            onClick={() => {
              setKind(item.kind);
            }}
          >
            {item.label}
            {item.template !== null && ' (own)'}
          </button>
        ))}
      </div>
      {current !== undefined && (
        <PromptEditor key={current.kind} prompt={current} version={prompts.data.version} />
      )}
    </section>
  );
}
