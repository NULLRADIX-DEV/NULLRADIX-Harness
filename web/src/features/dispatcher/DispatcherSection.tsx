import { useQuery } from '@tanstack/react-query';
import { useId, useMemo, useState, type ReactNode, type SubmitEvent } from 'react';
import { ApiProblemError } from '../../shared/api/errors';
import { problemMessage } from '../../shared/api/problemMessage';
import { FormError } from '../../shared/ui/FormError';
import { accountsQuery } from '../accounts/queries';
import { checkForm, toConfig, toForm, type DispatcherForm, type FieldErrors } from './configForm';
import {
  AreaSetting,
  CheckSetting,
  NumberSetting,
  SelectSetting,
  TextSetting,
  type AccountChoice,
} from './controls';
import {
  LadderEditor,
  ProfileOptions,
  ProfilesEditor,
  RulesEditor,
  SetupStepsEditor,
  profileNames,
  type Edit,
} from './editors';
import { PromptsEditor } from './PromptsEditor';
import {
  dispatcherConfigQuery,
  useSaveDispatcherConfig,
  type DispatcherConfigView,
} from './queries';
import { STEP_UP_HOW, StepUpPanel } from '../stepUp/StepUpPanel';
import './dispatcher.css';
import { Button } from '../../shared/ui/Button';
import { Panel } from '../../shared/ui/Panel';

export const CONFLICT_MESSAGE =
  'The dispatcher configuration changed in the meantime. Reload to see the new version.';
export const INVALID_MESSAGE = 'Some fields are not valid. They are marked below.';

const MESSAGES = {
  'version-conflict': CONFLICT_MESSAGE,
  'dispatcher-config-invalid': INVALID_MESSAGE,
  'step-up-required': `The fields that run commands are marked below. ${STEP_UP_HOW}`,
  'command-change-needs-person': 'Only the person in the app can change the fields marked below.',
};

function serverErrors(error: unknown): FieldErrors {
  if (!(error instanceof ApiProblemError)) return {};
  const found: Record<string, string> = {};
  for (const issue of error.fields) found[issue.field] ??= issue.message;
  return found;
}

function Group({ title, hint, children }: { title: string; hint?: string; children: ReactNode }) {
  return (
    <section className="dispatcher-group">
      <h3>{title}</h3>
      {hint !== undefined && <p className="field-hint muted">{hint}</p>}
      {children}
    </section>
  );
}

interface FormProps {
  form: DispatcherForm;
  errors: FieldErrors;
  accounts: readonly AccountChoice[];
  edit: Edit;
}

function Operation({ form, errors, edit }: FormProps) {
  const set = <K extends keyof DispatcherForm>(key: K, value: DispatcherForm[K]) => {
    edit((current) => ({ ...current, [key]: value }));
  };
  return (
    <>
      <Group
        title="Operation"
        hint="Off means nothing new starts; running runs are still evaluated."
      >
        <CheckSetting
          label="Dispatcher on"
          checked={form.enabled}
          onChange={(value) => {
            set('enabled', value);
          }}
        />
        <NumberSetting
          label="Parallel tickets (WIP)"
          hint="At most this many tickets are worked on at the same time."
          value={form.wip}
          error={errors['wip']}
          onChange={(value) => {
            set('wip', value);
          }}
        />
        <div className="form-row">
          <TextSetting
            label="Window from (HH:MM)"
            short
            placeholder="18:00"
            value={form.from}
            error={errors['window']}
            onChange={(value) => {
              set('from', value);
            }}
          />
          <TextSetting
            label="Window to (HH:MM)"
            short
            placeholder="07:00"
            value={form.to}
            onChange={(value) => {
              set('to', value);
            }}
          />
        </div>
        <p className="field-hint muted">
          New runs start only inside the window, in local time. A window that ends before it begins
          wraps past midnight.
        </p>
        <TextSetting
          label="Only tickets with label"
          placeholder="empty means all ready tickets"
          value={form.optInLabel}
          onChange={(value) => {
            set('optInLabel', value);
          }}
        />
        <TextSetting
          label="Never tickets with label"
          value={form.optOutLabel}
          onChange={(value) => {
            set('optOutLabel', value);
          }}
        />
        <NumberSetting
          label="Attempts per ticket"
          hint="After this many aborted runs a ticket is blocked."
          value={form.maxAttempts}
          error={errors['maxAttempts']}
          onChange={(value) => {
            set('maxAttempts', value);
          }}
        />
        <NumberSetting
          label="Pause after a usage limit (minutes)"
          value={form.pauseMinutesOnLimit}
          error={errors['pauseMinutesOnLimit']}
          onChange={(value) => {
            set('pauseMinutesOnLimit', value);
          }}
        />
      </Group>
      <Group title="Autopilot" hint="Tickets run without approvals; research answers questions.">
        <CheckSetting
          label="Autopilot on"
          checked={form.autopilotEnabled}
          onChange={(value) => {
            set('autopilotEnabled', value);
          }}
        />
        <TextSetting
          label="Autopilot exception label"
          placeholder="empty means no exception"
          value={form.autopilotExceptLabel}
          onChange={(value) => {
            set('autopilotExceptLabel', value);
          }}
        />
        <NumberSetting
          label="Merge sessions per conflict (1 to 5)"
          value={form.autopilotMaxMergeSessions}
          error={errors['autopilot.maxMergeSessions']}
          onChange={(value) => {
            set('autopilotMaxMergeSessions', value);
          }}
        />
      </Group>
      <Group title="Merging">
        <SelectSetting
          label="Merge strategy"
          value={form.mergeStrategy}
          onChange={(value) => {
            set('mergeStrategy', value === 'squash' ? 'squash' : 'merge');
          }}
        >
          <option value="merge">Merge commit (all commits stay)</option>
          <option value="squash">Squash (one commit per ticket)</option>
        </SelectSetting>
        <CheckSetting
          label="Wait for approval before merging"
          hint={
            form.autopilotEnabled
              ? 'The autopilot takes precedence for tickets it runs.'
              : 'Finished tickets wait until you accept them.'
          }
          checked={form.reviewBeforeMerge}
          onChange={(value) => {
            set('reviewBeforeMerge', value);
          }}
        />
        <SelectSetting
          label="Worktrees"
          value={form.joinWorktrees}
          onChange={(value) => {
            set('joinWorktrees', value === 'same-spec' ? 'same-spec' : 'none');
          }}
        >
          <option value="none">One worktree per ticket</option>
          <option value="same-spec">Tickets of one spec share a worktree (old)</option>
        </SelectSetting>
      </Group>
    </>
  );
}

function WorktreeSetup({ form, errors, edit }: FormProps) {
  return (
    <Group
      title="Worktree setup"
      hint="Commands run in a fresh worktree before Claude starts, one after the other. The folder is relative to the worktree root."
    >
      <SetupStepsEditor form={form} errors={errors} edit={edit} />
      <NumberSetting
        label="Setup timeout (minutes)"
        hint="The whole setup is stopped after this long."
        value={form.worktreeSetupTimeoutMinutes}
        error={errors['worktreeSetupTimeoutMinutes']}
        onChange={(value) => {
          edit((current) => ({ ...current, worktreeSetupTimeoutMinutes: value }));
        }}
      />
    </Group>
  );
}

function Profiles({
  form,
  errors,
  accounts,
  edit,
  base,
}: FormProps & { base: DispatcherConfigView }) {
  const names = profileNames(form);
  const set = <K extends keyof DispatcherForm>(key: K, value: DispatcherForm[K]) => {
    edit((current) => ({ ...current, [key]: value }));
  };
  return (
    <>
      <Group
        title="Profiles"
        hint="Model, effort and limits of a run, from cheap to expensive. Empty fields use the defaults of Claude Code."
      >
        <ProfilesEditor
          form={form}
          errors={errors}
          accounts={accounts}
          edit={edit}
          defaults={base.config}
        />
        <SelectSetting
          label="Default profile"
          hint="Used when no rule and no label picks one."
          value={form.defaultProfile}
          error={errors['defaultProfile']}
          onChange={(value) => {
            set('defaultProfile', value);
          }}
        >
          <ProfileOptions names={names} value={form.defaultProfile} empty="-" />
        </SelectSetting>
        <CheckSetting
          label="Escalate after an abort"
          hint="Each further attempt uses the next profile of the ladder."
          checked={form.escalate}
          onChange={(value) => {
            set('escalate', value);
          }}
        />
        <CheckSetting
          label="Allow Fable"
          hint="Profiles may use the Fable model."
          checked={form.allowFable}
          onChange={(value) => {
            set('allowFable', value);
          }}
        />
      </Group>
      <Group
        title="Escalation ladder"
        hint="Profiles from cheap to expensive, used when escalation is on."
      >
        <LadderEditor form={form} errors={errors} edit={edit} />
      </Group>
      <Group
        title="Rules"
        hint="The first matching rule picks the profile. All set conditions of a rule must hold."
      >
        <RulesEditor form={form} errors={errors} accounts={accounts} edit={edit} />
      </Group>
    </>
  );
}

function AccountSettings({ form, errors, edit }: FormProps) {
  const set = <K extends keyof DispatcherForm>(key: K, value: DispatcherForm[K]) => {
    edit((current) => ({ ...current, [key]: value }));
  };
  return (
    <Group title="Accounts" hint="How an account is picked among the allowed ones of a profile.">
      <SelectSetting
        label="Account choice"
        value={form.accountStrategy}
        onChange={(value) => {
          set('accountStrategy', value === 'order' ? 'order' : 'most-free');
        }}
      >
        <option value="most-free">Most free capacity</option>
        <option value="order">First free one of the list</option>
      </SelectSetting>
      <NumberSetting
        label="Account full from (percent)"
        value={form.accountLimitPercent}
        error={errors['accountLimitPercent']}
        onChange={(value) => {
          set('accountLimitPercent', value);
        }}
      />
      <NumberSetting
        label="Reserve for 5 hours (percent points)"
        value={form.accountReserveFiveHour}
        error={errors['accountReserveFiveHour']}
        onChange={(value) => {
          set('accountReserveFiveHour', value);
        }}
      />
      <NumberSetting
        label="Reserve for the week (percent points)"
        value={form.accountReserveWeek}
        error={errors['accountReserveWeek']}
        onChange={(value) => {
          set('accountReserveWeek', value);
        }}
      />
      <NumberSetting
        label="No reserve when 5 hours reset within (minutes)"
        value={form.accountResetGraceMinutes}
        error={errors['accountResetGraceMinutes']}
        onChange={(value) => {
          set('accountResetGraceMinutes', value);
        }}
      />
    </Group>
  );
}

function AutoRuns({ form, errors, edit }: FormProps) {
  const names = profileNames(form);
  const set = <K extends keyof DispatcherForm>(key: K, value: DispatcherForm[K]) => {
    edit((current) => ({ ...current, [key]: value }));
  };
  return (
    <>
      <Group title="Research" hint="Open questions are researched by a read-only Claude session.">
        <CheckSetting
          label="Research new questions automatically"
          checked={form.researchAuto}
          onChange={(value) => {
            set('researchAuto', value);
          }}
        />
        <NumberSetting
          label="Parallel research runs (1 to 5)"
          value={form.researchWip}
          error={errors['research.wip']}
          onChange={(value) => {
            set('researchWip', value);
          }}
        />
        <NumberSetting
          label="Automatic research runs per day"
          value={form.researchMaxPerDay}
          error={errors['research.maxPerDay']}
          onChange={(value) => {
            set('researchMaxPerDay', value);
          }}
        />
        <SelectSetting
          label="Research profile"
          value={form.researchProfile}
          error={errors['research.profile']}
          onChange={(value) => {
            set('researchProfile', value);
          }}
        >
          <ProfileOptions names={names} value={form.researchProfile} empty="Default profile" />
        </SelectSetting>
      </Group>
      <Group
        title="Planning"
        hint="Tickets are planned by a read-only Claude session; the plan lands on the ticket."
      >
        <CheckSetting
          label="Plan tickets that need it automatically"
          checked={form.planningAuto}
          onChange={(value) => {
            set('planningAuto', value);
          }}
        />
        <NumberSetting
          label="Parallel planning runs (1 to 5)"
          value={form.planningWip}
          error={errors['planning.wip']}
          onChange={(value) => {
            set('planningWip', value);
          }}
        />
        <CheckSetting
          label="Fill in the acceptance while planning"
          checked={form.planningAcceptance}
          onChange={(value) => {
            set('planningAcceptance', value);
          }}
        />
        <SelectSetting
          label="Planning profile"
          value={form.planningProfile}
          error={errors['planning.profile']}
          onChange={(value) => {
            set('planningProfile', value);
          }}
        >
          <ProfileOptions names={names} value={form.planningProfile} empty="Default profile" />
        </SelectSetting>
      </Group>
    </>
  );
}

function ClaudeSettings({ form, errors, edit }: FormProps) {
  const set = <K extends keyof DispatcherForm>(key: K, value: DispatcherForm[K]) => {
    edit((current) => ({ ...current, [key]: value }));
  };
  return (
    <Group title="Claude" hint="How the dispatcher calls Claude Code.">
      <TextSetting
        label="Claude command"
        hint="claude or a full path."
        mono
        value={form.command}
        error={errors['claude.command']}
        onChange={(value) => {
          set('command', value);
        }}
      />
      <TextSetting
        label="Permission mode"
        hint="Value of --permission-mode."
        value={form.permissionMode}
        error={errors['claude.permissionMode']}
        onChange={(value) => {
          set('permissionMode', value);
        }}
      />
      <TextSetting
        label="Git Bash path (Windows only)"
        hint="Empty means the dispatcher looks for it."
        mono
        value={form.gitBashPath}
        error={errors['claude.gitBashPath']}
        onChange={(value) => {
          set('gitBashPath', value);
        }}
      />
      <AreaSetting
        label="Allowed tools (one per line)"
        hint="What the agent may do without asking (--allowedTools)."
        rows={6}
        value={form.allowedTools}
        error={errors['claude.allowedTools'] ?? errors['claude.extraArgs']}
        onChange={(value) => {
          set('allowedTools', value);
        }}
      />
      <AreaSetting
        label="Extra text for every prompt"
        rows={3}
        value={form.promptExtra}
        onChange={(value) => {
          set('promptExtra', value);
        }}
      />
    </Group>
  );
}

function ConfigEditor({
  view,
  accounts,
}: {
  view: DispatcherConfigView;
  accounts: AccountChoice[];
}) {
  const [base, setBase] = useState(view);
  const pristine = useMemo(() => toForm(base.config), [base]);
  const [form, setForm] = useState(pristine);
  const [submitted, setSubmitted] = useState(false);
  const [saved, setSaved] = useState(false);
  const save = useSaveDispatcherConfig();
  const dirty = JSON.stringify(form) !== JSON.stringify(pristine);

  // a version from elsewhere replaces the form only while nothing is being edited
  if (view !== base && !dirty) {
    setBase(view);
    setForm(toForm(view.config));
  }
  const behind = view.version !== base.version;
  const errors: FieldErrors = {
    ...(submitted ? checkForm(form) : {}),
    ...serverErrors(save.error),
  };

  const edit: Edit = (update) => {
    setForm(update);
    setSaved(false);
    if (save.isError) save.reset();
  };

  function reload() {
    setBase(view);
    setForm(toForm(view.config));
    setSaved(false);
    save.reset();
  }

  function submit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    setSubmitted(true);
    if (Object.keys(checkForm(form)).length > 0) return;
    save.mutate(
      { version: base.version, config: toConfig(form, base.config) },
      {
        onSuccess: (next) => {
          setBase(next);
          setForm(toForm(next.config));
          setSubmitted(false);
          setSaved(true);
        },
      },
    );
  }

  const props: FormProps = { form, errors, accounts, edit };
  return (
    <form
      className="dispatcher-form"
      aria-label="Dispatcher configuration"
      onSubmit={submit}
      noValidate
    >
      {behind && (
        <p className="form-error" role="alert">
          The configuration was changed elsewhere.{' '}
          <Button type="button" onClick={reload}>
            Reload
          </Button>
        </p>
      )}
      <Operation {...props} />
      <WorktreeSetup {...props} />
      <Profiles {...props} base={base} />
      <AccountSettings {...props} />
      <AutoRuns {...props} />
      <ClaudeSettings {...props} />
      <Group title="Confirmation">
        <StepUpPanel />
      </Group>
      <FormError
        message={problemMessage(save.error, MESSAGES, 'Could not save the configuration.')}
      />
      <div className="dispatcher-footer">
        <Button type="submit" variant="primary" disabled={save.isPending || !dirty}>
          Save dispatcher settings
        </Button>
        <span role="status" className="muted">
          {saved ? 'Saved.' : dirty ? 'Unsaved changes.' : ''}
        </span>
      </div>
    </form>
  );
}

export function DispatcherSection() {
  const headingId = useId();
  const config = useQuery(dispatcherConfigQuery());
  const accounts = useQuery(accountsQuery());
  const choices: AccountChoice[] = (accounts.data?.items ?? []).map((account) => ({
    slug: account.slug,
    name: account.name,
    enabled: account.enabled,
  }));
  return (
    <Panel as="section" className="panel page-section" aria-labelledby={headingId}>
      <h2 id={headingId}>Dispatcher</h2>
      <p className="muted dispatcher-note">
        The dispatcher takes ready tickets of your boards and lets Claude work on them in their own
        worktrees.
      </p>
      {config.status === 'pending' && <p className="muted">Loading the dispatcher settings...</p>}
      {config.status === 'error' && (
        <p className="form-error">Could not load the dispatcher settings.</p>
      )}
      {config.status === 'success' && <ConfigEditor view={config.data} accounts={choices} />}
      {config.status === 'success' && <PromptsEditor />}
    </Panel>
  );
}
