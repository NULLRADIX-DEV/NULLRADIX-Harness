import { useId, type ReactNode } from 'react';

interface Common {
  label: string;
  hint?: string;
  error?: string | undefined;
}

function Notes({
  id,
  hint,
  error,
}: {
  id: string;
  hint?: string | undefined;
  error?: string | undefined;
}) {
  return (
    <>
      {hint !== undefined && (
        <p id={`${id}-hint`} className="field-hint muted">
          {hint}
        </p>
      )}
      {error !== undefined && (
        <p id={`${id}-error`} className="form-error" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

function describedBy(id: string, hint?: string, error?: string): string | undefined {
  const ids = [hint !== undefined ? `${id}-hint` : '', error !== undefined ? `${id}-error` : ''];
  const used = ids.filter((part) => part !== '').join(' ');
  return used === '' ? undefined : used;
}

export function TextSetting(
  props: Common & {
    value: string;
    onChange: (value: string) => void;
    placeholder?: string;
    short?: boolean;
    mono?: boolean;
  },
) {
  const id = useId();
  const classes = [props.short === true ? 'short' : '', props.mono === true ? 'mono' : ''];
  return (
    <div className="setting">
      <label htmlFor={id}>{props.label}</label>
      <input
        id={id}
        className={classes.join(' ').trim()}
        value={props.value}
        placeholder={props.placeholder}
        autoComplete="off"
        spellCheck={false}
        aria-invalid={props.error !== undefined}
        aria-describedby={describedBy(id, props.hint, props.error)}
        onChange={(event) => {
          props.onChange(event.target.value);
        }}
      />
      <Notes id={id} hint={props.hint} error={props.error} />
    </div>
  );
}

export function NumberSetting(
  props: Common & { value: string; onChange: (value: string) => void },
) {
  const id = useId();
  return (
    <div className="setting">
      <label htmlFor={id}>{props.label}</label>
      <input
        id={id}
        className="short"
        inputMode="numeric"
        value={props.value}
        autoComplete="off"
        aria-invalid={props.error !== undefined}
        aria-describedby={describedBy(id, props.hint, props.error)}
        onChange={(event) => {
          props.onChange(event.target.value);
        }}
      />
      <Notes id={id} hint={props.hint} error={props.error} />
    </div>
  );
}

export function CheckSetting(
  props: Common & { checked: boolean; onChange: (checked: boolean) => void; disabled?: boolean },
) {
  const id = useId();
  return (
    <div className="setting setting-check">
      <label htmlFor={id}>
        <input
          id={id}
          type="checkbox"
          checked={props.checked}
          disabled={props.disabled}
          aria-describedby={describedBy(id, props.hint, props.error)}
          onChange={(event) => {
            props.onChange(event.target.checked);
          }}
        />
        <span>{props.label}</span>
      </label>
      <Notes id={id} hint={props.hint} error={props.error} />
    </div>
  );
}

export function SelectSetting(
  props: Common & {
    value: string;
    onChange: (value: string) => void;
    children: ReactNode;
  },
) {
  const id = useId();
  return (
    <div className="setting">
      <label htmlFor={id}>{props.label}</label>
      <select
        id={id}
        value={props.value}
        aria-invalid={props.error !== undefined}
        aria-describedby={describedBy(id, props.hint, props.error)}
        onChange={(event) => {
          props.onChange(event.target.value);
        }}
      >
        {props.children}
      </select>
      <Notes id={id} hint={props.hint} error={props.error} />
    </div>
  );
}

export function AreaSetting(
  props: Common & { value: string; onChange: (value: string) => void; rows?: number },
) {
  const id = useId();
  return (
    <div className="setting wide">
      <label htmlFor={id}>{props.label}</label>
      <textarea
        id={id}
        rows={props.rows ?? 4}
        value={props.value}
        spellCheck={false}
        aria-describedby={describedBy(id, props.hint, props.error)}
        onChange={(event) => {
          props.onChange(event.target.value);
        }}
      />
      <Notes id={id} hint={props.hint} error={props.error} />
    </div>
  );
}

export interface AccountChoice {
  slug: string;
  name: string;
  enabled: boolean;
}

// Several accounts in the order they were ticked; ids no account has any more stay visible.
export function AccountPicker({
  label,
  value,
  accounts,
  emptyText,
  error,
  onChange,
}: {
  label: string;
  value: readonly string[];
  accounts: readonly AccountChoice[];
  emptyText: string;
  error?: string | undefined;
  onChange: (value: string[]) => void;
}) {
  const known = new Set(accounts.map((account) => account.slug));
  const choices: AccountChoice[] = [
    ...accounts,
    ...value
      .filter((slug) => !known.has(slug))
      .map((slug) => ({ slug, name: slug, enabled: false })),
  ];
  const name = (slug: string) => choices.find((choice) => choice.slug === slug)?.name ?? slug;
  const toggle = (slug: string) => {
    onChange(value.includes(slug) ? value.filter((item) => item !== slug) : [...value, slug]);
  };
  return (
    <details className="account-picker" role="group" aria-label={label}>
      <summary>{value.length === 0 ? emptyText : value.map(name).join(', ')}</summary>
      <div className="account-picker-menu">
        {choices.map((choice) => (
          <label key={choice.slug}>
            <input
              type="checkbox"
              checked={value.includes(choice.slug)}
              aria-label={`${label}: ${choice.name}`}
              onChange={() => {
                toggle(choice.slug);
              }}
            />
            <span>{choice.name}</span>
            {!known.has(choice.slug) ? (
              <small>unknown</small>
            ) : (
              !choice.enabled && <small>off</small>
            )}
          </label>
        ))}
        {value.length > 0 && (
          <button
            type="button"
            className="btn"
            onClick={() => {
              onChange([]);
            }}
          >
            Clear ({emptyText})
          </button>
        )}
      </div>
      {error !== undefined && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
    </details>
  );
}
