import { useQuery } from '@tanstack/react-query';
import { useId, useState, type SubmitEvent } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import {
  accountsQuery,
  useClearRunToken,
  useCreateAccount,
  useDeleteAccount,
  useSetRunToken,
  useUpdateAccount,
  type Account,
} from './queries';
import './accounts.css';

export const NAME_MAX_LENGTH = 60;
export const CONFLICT_MESSAGE = 'This account changed in the meantime. The list is up to date now.';
export const GONE_MESSAGE = 'This account no longer exists.';
export const LIMIT_MESSAGE = 'Enter a whole number from 1 to 100, or leave the field empty.';

const MESSAGES = {
  'version-conflict': CONFLICT_MESSAGE,
  'account-not-found': GONE_MESSAGE,
  'run-token-invalid':
    'This does not look like a token of `claude setup-token`. It starts with sk-ant-oat.',
  'secret-store-failed': 'The system secret store did not answer. Try again in a moment.',
};

function AddAccountForm() {
  const [name, setName] = useState('');
  const [dir, setDir] = useState('');
  const create = useCreateAccount();

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    create.mutate(
      { name, ...(dir.trim() === '' ? {} : { configDir: dir.trim() }) },
      {
        onSuccess: () => {
          setName('');
          setDir('');
        },
      },
    );
  }

  return (
    <form className="account-add" aria-label="New account" onSubmit={submit}>
      <div className="form-row">
        <label className="field account-name-field">
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
          disabled={create.isPending || name.trim() === ''}
        >
          Add account
        </button>
      </div>
      <details className="account-more">
        <summary>Use an existing Claude folder</summary>
        <label className="field">
          <span>Folder</span>
          <input
            className="mono"
            value={dir}
            placeholder="~/.mcacc/work/claude"
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => {
              setDir(event.target.value);
            }}
          />
        </label>
        <p className="field-hint muted">
          For a folder where Claude Code is already logged in. Without one, the account gets its own
          folder.
        </p>
      </details>
      <FormError message={problemMessage(create.error, MESSAGES, 'Could not add the account.')} />
    </form>
  );
}

function LimitInput({
  label,
  value,
  disabled,
  onSave,
}: {
  label: string;
  value: number | null | undefined;
  disabled: boolean;
  onSave: (limit: number | null) => void;
}) {
  const shown = value === null || value === undefined ? '' : String(value);
  const [text, setText] = useState(shown);
  const [invalid, setInvalid] = useState(false);
  const [seen, setSeen] = useState(shown);

  // follow the stored value when it changes elsewhere
  if (seen !== shown) {
    setSeen(shown);
    setText(shown);
    setInvalid(false);
  }

  function save() {
    const trimmed = text.trim();
    if (trimmed === shown) {
      setInvalid(false);
      return;
    }
    const limit = trimmed === '' ? null : Number(trimmed);
    if (limit !== null && !(Number.isInteger(limit) && limit >= 1 && limit <= 100)) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    onSave(limit);
  }

  return (
    <label className="account-limit">
      <span>{label}</span>
      <input
        className="short"
        inputMode="numeric"
        value={text}
        placeholder="-"
        disabled={disabled}
        aria-invalid={invalid}
        aria-label={`${label} limit in percent`}
        onChange={(event) => {
          setText(event.target.value);
        }}
        onBlur={save}
        onKeyDown={(event) => {
          if (event.key === 'Enter') save();
        }}
      />
      <span>%</span>
      {invalid && (
        <span className="form-error" role="alert">
          {LIMIT_MESSAGE}
        </span>
      )}
    </label>
  );
}

function RunToken({ account }: { account: Account }) {
  const [token, setToken] = useState('');
  const set = useSetRunToken();
  const clear = useClearRunToken();
  const busy = set.isPending || clear.isPending;

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    set.mutate(
      { id: account.id, token: token.trim(), version: account.version },
      {
        onSuccess: () => {
          setToken('');
          set.reset();
        },
      },
    );
  }

  if (account.hasRunToken) {
    return (
      <div className="account-token">
        <span>Run token</span>
        <span className="account-badge">set</span>
        <button
          type="button"
          className="btn"
          disabled={busy}
          aria-label={`Remove the run token of ${account.name}`}
          onClick={() => {
            clear.mutate({ id: account.id, version: account.version });
          }}
        >
          Remove
        </button>
        <FormError
          message={problemMessage(clear.error, MESSAGES, 'Could not remove the run token.')}
        />
      </div>
    );
  }
  return (
    <form className="account-token" aria-label={`Run token of ${account.name}`} onSubmit={submit}>
      <label className="field account-token-field">
        <span>Run token</span>
        <input
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={token}
          placeholder="sk-ant-oat01-..."
          onChange={(event) => {
            setToken(event.target.value);
          }}
        />
      </label>
      <button type="submit" className="btn" disabled={busy || token.trim() === ''}>
        Save token
      </button>
      <p className="field-hint muted">
        Create it in a terminal with <code>claude setup-token</code>. It is kept in the system
        secret store and never shown again.
      </p>
      <FormError message={problemMessage(set.error, MESSAGES, 'Could not store the run token.')} />
    </form>
  );
}

function RenameForm({ account, onDone }: { account: Account; onDone: () => void }) {
  const [name, setName] = useState(account.name);
  const update = useUpdateAccount();
  const trimmed = name.trim();

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    update.mutate({ id: account.id, name, version: account.version }, { onSuccess: onDone });
  }

  return (
    <form className="project-edit" aria-label={`Rename ${account.name}`} onSubmit={submit}>
      <div className="form-row">
        <label className="field account-name-field">
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
        <button
          type="submit"
          className="btn primary"
          disabled={update.isPending || trimmed === '' || trimmed === account.name}
        >
          Save
        </button>
        <button type="button" className="btn" onClick={onDone}>
          Cancel
        </button>
      </div>
      <FormError
        message={problemMessage(update.error, MESSAGES, 'Could not rename the account.')}
      />
    </form>
  );
}

function RemoveConfirm({
  account,
  accountsDir,
  onDone,
}: {
  account: Account;
  accountsDir: string;
  onDone: () => void;
}) {
  const [deleteDir, setDeleteDir] = useState(false);
  const remove = useDeleteAccount();
  const own = account.configDir?.startsWith(accountsDir) === true;

  return (
    <div
      className="project-confirm"
      role="group"
      aria-label={`Remove ${account.name}`}
      onKeyDown={(event) => {
        if (event.key === 'Escape') onDone();
      }}
    >
      <p>
        Remove <strong>{account.name}</strong>? Its run token is forgotten. This cannot be undone.
      </p>
      {own && (
        <label className="account-check">
          <input
            type="checkbox"
            checked={deleteDir}
            onChange={(event) => {
              setDeleteDir(event.target.checked);
            }}
          />
          <span>Also delete its Claude folder with the login</span>
        </label>
      )}
      <div className="form-row">
        <button
          type="button"
          className="btn danger"
          disabled={remove.isPending}
          onClick={() => {
            remove.mutate(
              { id: account.id, version: account.version, deleteDir: own && deleteDir },
              { onSuccess: onDone },
            );
          }}
        >
          Remove
        </button>
        <button type="button" className="btn" onClick={onDone}>
          Cancel
        </button>
      </div>
      <FormError
        message={problemMessage(remove.error, MESSAGES, 'Could not remove the account.')}
      />
    </div>
  );
}

type RowMode = 'view' | 'rename' | 'remove';

function AccountRow({ account, accountsDir }: { account: Account; accountsDir: string }) {
  const [mode, setMode] = useState<RowMode>('view');
  const update = useUpdateAccount();
  const back = () => {
    setMode('view');
  };

  return (
    <li className="account-row" aria-label={account.name}>
      <div className="project-line">
        <span className="project-key mono">{account.slug}</span>
        <span className="project-name">{account.name}</span>
        {account.configDir === null || account.configDir === undefined ? (
          <span className="account-badge">server login</span>
        ) : null}
        {!account.enabled && <span className="account-badge">disabled</span>}
        {account.enabled && account.fallback && <span className="account-badge">fallback</span>}
        {mode === 'view' && (
          <span className="project-actions">
            <button
              type="button"
              className="btn"
              aria-label={`Rename ${account.name}`}
              onClick={() => {
                setMode('rename');
              }}
            >
              Rename
            </button>
            {account.slug !== 'default' && (
              <button
                type="button"
                className="btn"
                aria-label={`Remove ${account.name}`}
                onClick={() => {
                  setMode('remove');
                }}
              >
                Remove
              </button>
            )}
          </span>
        )}
      </div>
      {account.configDir && <p className="account-dir mono muted">{account.configDir}</p>}
      {mode === 'rename' && <RenameForm account={account} onDone={back} />}
      {mode === 'remove' && (
        <RemoveConfirm account={account} accountsDir={accountsDir} onDone={back} />
      )}
      <div className="account-settings">
        <label className="account-check">
          <input
            type="checkbox"
            checked={account.enabled}
            disabled={update.isPending}
            onChange={(event) => {
              update.mutate({
                id: account.id,
                enabled: event.target.checked,
                version: account.version,
              });
            }}
          />
          <span>Enabled</span>
        </label>
        <label className="account-check">
          <input
            type="checkbox"
            checked={account.fallback}
            disabled={update.isPending || !account.enabled}
            onChange={(event) => {
              update.mutate({
                id: account.id,
                fallback: event.target.checked,
                version: account.version,
              });
            }}
          />
          <span>Only as fallback</span>
        </label>
        <LimitInput
          label="5 hours"
          value={account.limitFiveHour}
          disabled={update.isPending}
          onSave={(limit) => {
            update.mutate({ id: account.id, limitFiveHour: limit, version: account.version });
          }}
        />
        <LimitInput
          label="Week"
          value={account.limitWeek}
          disabled={update.isPending}
          onSave={(limit) => {
            update.mutate({ id: account.id, limitWeek: limit, version: account.version });
          }}
        />
      </div>
      <FormError message={problemMessage(update.error, MESSAGES, 'Could not save the account.')} />
      <RunToken account={account} />
    </li>
  );
}

function AccountList() {
  const { status, data } = useQuery(accountsQuery());
  if (status === 'pending') return <p className="muted">Loading accounts...</p>;
  if (status === 'error') return <p className="form-error">Could not load the accounts.</p>;
  return (
    <ul className="project-list" aria-label="Accounts">
      {data.items.map((account) => (
        <AccountRow key={account.id} account={account} accountsDir={data.accountsDir} />
      ))}
    </ul>
  );
}

export function AccountsSection() {
  const headingId = useId();
  return (
    <section className="panel page-section" aria-labelledby={headingId}>
      <h2 id={headingId}>Claude accounts</h2>
      <p className="muted account-note">
        Runs can be spread over several Claude logins. Using several subscriptions to get around the
        limits of one may break Anthropic&apos;s terms, so use accounts meant for it, such as team
        seats.
      </p>
      <AddAccountForm />
      <AccountList />
    </section>
  );
}
