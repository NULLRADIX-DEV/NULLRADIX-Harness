import { useQuery } from '@tanstack/react-query';
import { useId, useState, type SubmitEvent } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import {
  accountsQuery,
  usageQuery,
  useClearRunToken,
  useCreateAccount,
  useDeleteAccount,
  useSendLoginCode,
  useSetRunToken,
  useStartLogin,
  useCancelLogin,
  useCreateAccountLogin,
  useUpdateAccount,
  type Account,
  type AccountStatus,
  type LoginStatus,
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
  const [email, setEmail] = useState('');
  const [dir, setDir] = useState('');
  const create = useCreateAccount();
  const createWithLogin = useCreateAccountLogin();
  const busy = create.isPending || createWithLogin.isPending;

  function reset() {
    setName('');
    setEmail('');
    setDir('');
  }

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    create.mutate(
      { name, ...(dir.trim() === '' ? {} : { configDir: dir.trim() }) },
      { onSuccess: reset },
    );
  }

  function addAndLogIn() {
    createWithLogin.mutate(
      {
        ...(name.trim() === '' ? {} : { name }),
        ...(email.trim() === '' ? {} : { email: email.trim() }),
      },
      { onSuccess: reset },
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
        <label className="field account-name-field">
          <span>Email</span>
          <input
            type="email"
            value={email}
            placeholder="me@example.com"
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => {
              setEmail(event.target.value);
            }}
          />
        </label>
        <button type="submit" className="btn" disabled={busy || name.trim() === ''}>
          Add account
        </button>
        <button type="button" className="btn primary" disabled={busy} onClick={addAndLogIn}>
          Add and log in with Claude
        </button>
      </div>
      <p className="field-hint muted">
        The email is optional and fills the login page. Without a name, the account is named after
        the email once it is logged in.
      </p>
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
      <FormError
        message={problemMessage(createWithLogin.error, MESSAGES, 'Could not add the account.')}
      />
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

export const REASONS = {
  disabled: 'No limits are read while the account is disabled.',
  'no-login': "Not logged in. Log in with Claude Code using this account's folder.",
  'token-expired': 'The login expired. Log in again with Claude Code.',
  'request-failed': 'Anthropic did not answer. Trying again soon.',
  'rate-limited': 'Anthropic asked to wait. Trying again later.',
} as const;

function resetTime(resetsAt: number | null | undefined): string | null {
  if (resetsAt === null || resetsAt === undefined) return null;
  return new Date(resetsAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function Meter({
  label,
  window,
  limit,
}: {
  label: string;
  window: { percent: number; resetsAt?: number | null } | null | undefined;
  limit: number | null | undefined;
}) {
  if (!window) {
    return (
      <span className="account-meter">
        {label} <span className="muted">-</span>
      </span>
    );
  }
  const reset = resetTime(window.resetsAt);
  const over = limit !== null && limit !== undefined && window.percent >= limit;
  return (
    <span className={`account-meter${over ? ' over' : ''}`}>
      {label}{' '}
      <meter
        min={0}
        max={100}
        value={Math.min(window.percent, 100)}
        aria-label={`${label} usage`}
      />{' '}
      <strong>{String(window.percent)}%</strong>
      {reset !== null && <span className="muted"> resets {reset}</span>}
    </span>
  );
}

// What Claude Code printed on a failed login, without the page, the prompt and the progress lines.
export function failureText(output: string): string {
  return output
    .split(/\r?\n/)
    .map((line) => line.replace(/^.*Paste code here if prompted >\s*/, ''))
    .filter(
      (line) =>
        line.trim() !== '' && !/https?:\/\//.test(line) && !line.startsWith('Opening browser'),
    )
    .slice(-6)
    .join('\n');
}

function LoginBox({
  account,
  login,
  loggedIn,
}: {
  account: Account;
  login: LoginStatus | null | undefined;
  loggedIn: boolean;
}) {
  const [code, setCode] = useState('');
  const start = useStartLogin();
  const send = useSendLoginCode();
  const cancel = useCancelLogin();
  if (!login || login.state === 'done' || login.state === 'cancelled') {
    return (
      <div className="account-login">
        <button
          type="button"
          className="btn"
          disabled={start.isPending || !account.enabled}
          aria-label={`Log in ${account.name} with Claude`}
          onClick={() => {
            start.mutate({ id: account.id });
          }}
        >
          {loggedIn ? 'Log in again' : 'Log in with Claude'}
        </button>
        <FormError message={problemMessage(start.error, MESSAGES, 'Could not start the login.')} />
      </div>
    );
  }

  if (login.state === 'failed') {
    const reason = failureText(login.output);
    return (
      <div className="account-login" role="group" aria-label={`Login of ${account.name}`}>
        <p className="form-error">The login failed.</p>
        {reason !== '' && <pre className="account-login-output">{reason}</pre>}
        <button
          type="button"
          className="btn"
          disabled={start.isPending}
          onClick={() => {
            start.mutate({ id: account.id });
          }}
        >
          Try again
        </button>
        <FormError message={problemMessage(start.error, MESSAGES, 'Could not start the login.')} />
      </div>
    );
  }

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    send.mutate(
      { id: account.id, code: code.trim() },
      {
        onSuccess: () => {
          setCode('');
        },
      },
    );
  }

  return (
    <form
      className="account-login"
      role="group"
      aria-label={`Login of ${account.name}`}
      onSubmit={submit}
    >
      <p>
        Log in with this account in the browser. Claude then shows a code: paste it here and send
        it.
      </p>
      {login.url && (
        <p className="field-hint muted">
          No window opened?{' '}
          <a href={login.url} target="_blank" rel="noreferrer">
            Open the login page
          </a>
        </p>
      )}
      <div className="form-row">
        <label className="field account-token-field">
          <span className="visually-hidden">Code from the browser</span>
          <input
            value={code}
            autoComplete="off"
            spellCheck={false}
            placeholder="Code from the browser"
            aria-label="Code from the browser"
            onChange={(event) => {
              setCode(event.target.value);
            }}
          />
        </label>
        <button type="submit" className="btn" disabled={send.isPending || code.trim() === ''}>
          Send code
        </button>
        <button
          type="button"
          className="btn"
          disabled={cancel.isPending}
          onClick={() => {
            cancel.mutate(account.id);
          }}
        >
          Cancel login
        </button>
      </div>
      <FormError message={problemMessage(send.error, MESSAGES, 'Could not send the code.')} />
    </form>
  );
}

// Login, plan and limits of an account; without numbers the reason says why.
function UsageLine({ account, status }: { account: Account; status: AccountStatus | undefined }) {
  if (!status) return null;
  const { usage } = status;
  const reason = usage.reason ? REASONS[usage.reason] : null;
  return (
    <div className="account-usage" aria-label={`Usage of ${account.name}`}>
      {status.email && <span className="mono">{status.email}</span>}
      {usage.plan && <span className="account-badge">{usage.plan}</span>}
      {!status.loggedIn && account.enabled && <span className="account-badge">not logged in</span>}
      {reason === null ? (
        <>
          <Meter label="5 hours" window={usage.fiveHour} limit={account.limitFiveHour} />
          <Meter label="Week" window={usage.sevenDay} limit={account.limitWeek} />
        </>
      ) : (
        <span className="muted">{reason}</span>
      )}
    </div>
  );
}

type RowMode = 'view' | 'rename' | 'remove';

function AccountRow({
  account,
  accountsDir,
  status,
}: {
  account: Account;
  accountsDir: string;
  status: AccountStatus | undefined;
}) {
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
      <UsageLine account={account} status={status} />
      <LoginBox account={account} login={status?.login} loggedIn={status?.loggedIn ?? false} />
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
  const { data: usage } = useQuery(usageQuery());
  if (status === 'pending') return <p className="muted">Loading accounts...</p>;
  if (status === 'error') return <p className="form-error">Could not load the accounts.</p>;
  return (
    <ul className="project-list" aria-label="Accounts">
      {data.items.map((account) => (
        <AccountRow
          key={account.id}
          account={account}
          accountsDir={data.accountsDir}
          status={usage?.find((entry) => entry.accountId === account.id)}
        />
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
