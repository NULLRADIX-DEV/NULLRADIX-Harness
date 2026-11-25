import type { ReactNode } from 'react';
import {
  EFFORT_LEVELS,
  addLadderStep,
  addProfile,
  addRule,
  changeProfile,
  moveLadderStep,
  moveProfile,
  moveRule,
  removeProfile,
  type DispatcherForm,
  type FieldErrors,
} from './configForm';
import { AccountPicker, type AccountChoice } from './controls';
import type { DispatcherConfig } from './queries';

export type Edit = (update: (form: DispatcherForm) => DispatcherForm) => void;

interface EditorProps {
  form: DispatcherForm;
  errors: FieldErrors;
  accounts: readonly AccountChoice[];
  edit: Edit;
}

// Messages of every field below `prefix`, in the order of the fields.
function messages(errors: FieldErrors, prefix: string): string[] {
  return Object.entries(errors)
    .filter(
      ([field]) =>
        field === prefix || field.startsWith(`${prefix}.`) || field.startsWith(`${prefix}[`),
    )
    .map(
      ([field, message]) =>
        `${field.slice(prefix.length).replace(/^\./, '') || 'Value'}: ${message}`,
    );
}

function RowErrors({ errors, prefix }: { errors: FieldErrors; prefix: string }) {
  const found = messages(errors, prefix);
  if (found.length === 0) return null;
  return (
    <ul className="dispatcher-row-errors">
      {found.map((message) => (
        <li key={message} className="form-error" role="alert">
          {message}
        </li>
      ))}
    </ul>
  );
}

export function ProfileOptions({
  names,
  value,
  empty,
}: {
  names: readonly string[];
  value: string;
  empty?: string;
}) {
  const unknown = value.trim() !== '' && !names.includes(value.trim());
  return (
    <>
      {empty !== undefined && <option value="">{empty}</option>}
      {unknown && <option value={value}>{`(missing: ${value})`}</option>}
      {names.map((name) => (
        <option key={name} value={name}>
          {name}
        </option>
      ))}
    </>
  );
}

export function profileNames(form: DispatcherForm): string[] {
  return [
    ...new Set(form.profiles.map((profile) => profile.name.trim()).filter((name) => name !== '')),
  ];
}

function Move({
  label,
  at,
  count,
  onMove,
}: {
  label: string;
  at: number;
  count: number;
  onMove: (to: number) => void;
}) {
  return (
    <>
      <button
        type="button"
        className="btn"
        disabled={at === 0}
        aria-label={`Move ${label} up`}
        onClick={() => {
          onMove(at - 1);
        }}
      >
        Up
      </button>
      <button
        type="button"
        className="btn"
        disabled={at === count - 1}
        aria-label={`Move ${label} down`}
        onClick={() => {
          onMove(at + 1);
        }}
      >
        Down
      </button>
    </>
  );
}

export function ProfilesEditor({
  form,
  errors,
  accounts,
  edit,
  defaults,
}: EditorProps & { defaults: DispatcherConfig }) {
  return (
    <div className="dispatcher-table">
      <div className="dispatcher-row profile head" aria-hidden="true">
        <span>Name</span>
        <span>Model</span>
        <span>Effort</span>
        <span>Turns</span>
        <span>Minutes</span>
        <span>Accounts</span>
        <span />
      </div>
      <ul className="dispatcher-rows" aria-label="Profiles">
        {form.profiles.map((profile, at) => {
          const label = `profile ${String(at + 1)}`;
          const path = `profiles[${String(at)}]`;
          const set = (patch: Parameters<typeof changeProfile>[2]) => {
            edit((current) => changeProfile(current, profile.key, patch));
          };
          return (
            <li key={profile.key} className="dispatcher-item">
              <div className="dispatcher-row profile">
                <input
                  aria-label={`Profile ${String(at + 1)} name`}
                  value={profile.name}
                  autoComplete="off"
                  onChange={(event) => {
                    set({ name: event.target.value });
                  }}
                />
                <input
                  aria-label={`Profile ${String(at + 1)} model`}
                  value={profile.model}
                  placeholder="sonnet, opus ..."
                  autoComplete="off"
                  onChange={(event) => {
                    set({ model: event.target.value });
                  }}
                />
                <select
                  aria-label={`Profile ${String(at + 1)} effort`}
                  value={profile.effort}
                  onChange={(event) => {
                    set({ effort: event.target.value });
                  }}
                >
                  <option value="">Default</option>
                  {EFFORT_LEVELS.map((level) => (
                    <option key={level} value={level}>
                      {level}
                    </option>
                  ))}
                </select>
                <input
                  aria-label={`Profile ${String(at + 1)} turns`}
                  inputMode="numeric"
                  value={profile.maxTurns}
                  onChange={(event) => {
                    set({ maxTurns: event.target.value });
                  }}
                />
                <input
                  aria-label={`Profile ${String(at + 1)} minutes`}
                  inputMode="numeric"
                  value={profile.timeoutMinutes}
                  onChange={(event) => {
                    set({ timeoutMinutes: event.target.value });
                  }}
                />
                <AccountPicker
                  label={`Profile ${String(at + 1)} accounts`}
                  value={profile.accounts}
                  accounts={accounts}
                  emptyText="any"
                  onChange={(value) => {
                    set({ accounts: value });
                  }}
                />
                <span className="dispatcher-actions">
                  <Move
                    label={label}
                    at={at}
                    count={form.profiles.length}
                    onMove={(to) => {
                      edit((current) => moveProfile(current, at, to));
                    }}
                  />
                  <button
                    type="button"
                    className="btn"
                    aria-label={`Remove ${label}`}
                    onClick={() => {
                      edit((current) => removeProfile(current, profile.key));
                    }}
                  >
                    Remove
                  </button>
                </span>
              </div>
              <RowErrors errors={errors} prefix={path} />
            </li>
          );
        })}
      </ul>
      <div>
        <button
          type="button"
          className="btn"
          onClick={() => {
            edit((current) => addProfile(current, defaults));
          }}
        >
          Add profile
        </button>
      </div>
    </div>
  );
}

export function LadderEditor({ form, errors, edit }: Omit<EditorProps, 'accounts'>) {
  const names = profileNames(form);
  return (
    <div className="dispatcher-table">
      <ol className="dispatcher-rows" aria-label="Escalation ladder">
        {form.ladder.map((step, at) => {
          const label = `step ${String(at + 1)}`;
          return (
            <li key={step.key} className="dispatcher-item">
              <div className="dispatcher-row ladder">
                <span className="muted">{at + 1}.</span>
                <select
                  aria-label={`Ladder ${label} profile`}
                  value={step.profile}
                  onChange={(event) => {
                    const profile = event.target.value;
                    edit((current) => ({
                      ...current,
                      ladder: current.ladder.map((item) =>
                        item.key === step.key ? { ...item, profile } : item,
                      ),
                    }));
                  }}
                >
                  <ProfileOptions names={names} value={step.profile} empty="-" />
                </select>
                <span className="dispatcher-actions">
                  <Move
                    label={`ladder ${label}`}
                    at={at}
                    count={form.ladder.length}
                    onMove={(to) => {
                      edit((current) => moveLadderStep(current, at, to));
                    }}
                  />
                  <button
                    type="button"
                    className="btn"
                    aria-label={`Remove ladder ${label}`}
                    onClick={() => {
                      edit((current) => ({
                        ...current,
                        ladder: current.ladder.filter((item) => item.key !== step.key),
                      }));
                    }}
                  >
                    Remove
                  </button>
                </span>
              </div>
              <RowErrors errors={errors} prefix={`ladder[${String(at)}]`} />
            </li>
          );
        })}
      </ol>
      <div>
        <button
          type="button"
          className="btn"
          onClick={() => {
            edit(addLadderStep);
          }}
        >
          Add step
        </button>
      </div>
    </div>
  );
}

export function RulesEditor({ form, errors, accounts, edit }: EditorProps) {
  const names = profileNames(form);
  return (
    <div className="dispatcher-table">
      <div className="dispatcher-row rule head" aria-hidden="true">
        <span>Labels (any)</span>
        <span>Priority</span>
        <span>Files from</span>
        <span>Plan chars from</span>
        <span>Profile</span>
        <span>Accounts</span>
        <span />
      </div>
      <ul className="dispatcher-rows" aria-label="Rules">
        {form.rules.map((rule, at) => {
          const n = String(at + 1);
          const set = (patch: Partial<Omit<(typeof form.rules)[number], 'key'>>) => {
            edit((current) => ({
              ...current,
              rules: current.rules.map((item) =>
                item.key === rule.key ? { ...item, ...patch } : item,
              ),
            }));
          };
          const field = (
            label: string,
            value: string,
            patch: (value: string) => Parameters<typeof set>[0],
            options: { placeholder?: string; numeric?: boolean } = {},
          ): ReactNode => (
            <input
              aria-label={`Rule ${n} ${label}`}
              value={value}
              placeholder={options.placeholder}
              inputMode={options.numeric === true ? 'numeric' : undefined}
              autoComplete="off"
              onChange={(event) => {
                set(patch(event.target.value));
              }}
            />
          );
          return (
            <li key={rule.key} className="dispatcher-item">
              <div className="dispatcher-row rule">
                {field('labels', rule.labels, (labels) => ({ labels }), {
                  placeholder: 'docs, typo',
                })}
                {field('priorities', rule.priorities, (priorities) => ({ priorities }), {
                  placeholder: 'urgent',
                })}
                {field('files', rule.filesMin, (filesMin) => ({ filesMin }), { numeric: true })}
                {field('plan characters', rule.planMinChars, (planMinChars) => ({ planMinChars }), {
                  numeric: true,
                })}
                <select
                  aria-label={`Rule ${n} profile`}
                  value={rule.profile}
                  onChange={(event) => {
                    set({ profile: event.target.value });
                  }}
                >
                  <ProfileOptions names={names} value={rule.profile} empty="-" />
                </select>
                <AccountPicker
                  label={`Rule ${n} accounts`}
                  value={rule.accounts}
                  accounts={accounts}
                  emptyText="as profile"
                  onChange={(value) => {
                    set({ accounts: value });
                  }}
                />
                <span className="dispatcher-actions">
                  <Move
                    label={`rule ${n}`}
                    at={at}
                    count={form.rules.length}
                    onMove={(to) => {
                      edit((current) => moveRule(current, at, to));
                    }}
                  />
                  <button
                    type="button"
                    className="btn"
                    aria-label={`Remove rule ${n}`}
                    onClick={() => {
                      edit((current) => ({
                        ...current,
                        rules: current.rules.filter((item) => item.key !== rule.key),
                      }));
                    }}
                  >
                    Remove
                  </button>
                </span>
              </div>
              <RowErrors errors={errors} prefix={`rules[${String(at)}]`} />
            </li>
          );
        })}
      </ul>
      <div>
        <button
          type="button"
          className="btn"
          onClick={() => {
            edit(addRule);
          }}
        >
          Add rule
        </button>
      </div>
    </div>
  );
}
