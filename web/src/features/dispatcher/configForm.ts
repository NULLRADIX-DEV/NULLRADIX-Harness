import type { DispatcherConfig, ProfileDto, RuleDto } from './queries';

export const EFFORT_LEVELS = ['low', 'medium', 'high', 'xhigh', 'max'] as const;

// Numbers and lists stay text while typing, so half-typed input never breaks the form.
export interface ProfileRow {
  key: number;
  // Stored id; new rows have none and the daemon gives them one.
  id: string | null;
  name: string;
  // The name references to this profile currently use; a rename carries them along.
  ref: string;
  model: string;
  effort: string;
  maxTurns: string;
  timeoutMinutes: string;
  accounts: string[];
}

export interface RuleRow {
  key: number;
  id: string | null;
  labels: string;
  priorities: string;
  filesMin: string;
  planMinChars: string;
  profile: string;
  accounts: string[];
}

export interface LadderStep {
  key: number;
  profile: string;
}

export interface DispatcherForm {
  enabled: boolean;
  wip: string;
  from: string;
  to: string;
  optInLabel: string;
  optOutLabel: string;
  maxAttempts: string;
  pauseMinutesOnLimit: string;
  mergeStrategy: 'merge' | 'squash';
  reviewBeforeMerge: boolean;
  joinWorktrees: 'none' | 'same-spec';
  profiles: ProfileRow[];
  defaultProfile: string;
  escalate: boolean;
  allowFable: boolean;
  ladder: LadderStep[];
  rules: RuleRow[];
  accountStrategy: 'most-free' | 'order';
  accountLimitPercent: string;
  accountReserveFiveHour: string;
  accountReserveWeek: string;
  accountResetGraceMinutes: string;
  researchAuto: boolean;
  researchWip: string;
  researchMaxPerDay: string;
  researchProfile: string;
  planningAuto: boolean;
  planningWip: string;
  planningAcceptance: boolean;
  planningProfile: string;
  autopilotEnabled: boolean;
  autopilotExceptLabel: string;
  autopilotMaxMergeSessions: string;
  command: string;
  permissionMode: string;
  gitBashPath: string;
  allowedTools: string;
  promptExtra: string;
}

export type FieldErrors = Readonly<Record<string, string>>;

const text = (value: string | number | null | undefined): string =>
  value === null || value === undefined ? '' : String(value);

const list = (value: string, separator: RegExp): string[] =>
  value
    .split(separator)
    .map((part) => part.trim())
    .filter((part) => part !== '');

const optional = (value: string): string | null => (value.trim() === '' ? null : value.trim());

const number = (value: string): number => (value.trim() === '' ? Number.NaN : Number(value));

export function toForm(config: DispatcherConfig): DispatcherForm {
  return {
    enabled: config.enabled,
    wip: text(config.wip),
    from: config.window?.from ?? '',
    to: config.window?.to ?? '',
    optInLabel: text(config.optInLabel),
    optOutLabel: text(config.optOutLabel),
    maxAttempts: text(config.maxAttempts),
    pauseMinutesOnLimit: text(config.pauseMinutesOnLimit),
    mergeStrategy: config.mergeStrategy,
    reviewBeforeMerge: config.reviewBeforeMerge,
    joinWorktrees: config.joinWorktrees,
    profiles: config.profiles.map((profile, key) => ({
      key,
      id: profile.id ?? null,
      name: profile.name,
      ref: profile.name,
      model: text(profile.model),
      effort: text(profile.effort),
      maxTurns: text(profile.maxTurns),
      timeoutMinutes: text(profile.timeoutMinutes),
      accounts: profile.accounts,
    })),
    defaultProfile: config.defaultProfile,
    escalate: config.escalate,
    allowFable: config.allowFable,
    ladder: config.ladder.map((profile, key) => ({ key, profile })),
    rules: config.rules.map((rule, key) => ({
      key,
      id: rule.id ?? null,
      labels: (rule.when.labels ?? []).join(', '),
      priorities: (rule.when.priorities ?? []).join(', '),
      filesMin: text(rule.when.filesMin),
      planMinChars: text(rule.when.planMinChars),
      profile: rule.profile,
      accounts: rule.accounts,
    })),
    accountStrategy: config.accountStrategy,
    accountLimitPercent: text(config.accountLimitPercent),
    accountReserveFiveHour: text(config.accountReserveFiveHour),
    accountReserveWeek: text(config.accountReserveWeek),
    accountResetGraceMinutes: text(config.accountResetGraceMinutes),
    researchAuto: config.research.auto,
    researchWip: text(config.research.wip),
    researchMaxPerDay: text(config.research.maxPerDay),
    researchProfile: text(config.research.profile),
    planningAuto: config.planning.auto,
    planningWip: text(config.planning.wip),
    planningAcceptance: config.planning.acceptance,
    planningProfile: text(config.planning.profile),
    autopilotEnabled: config.autopilot.enabled,
    autopilotExceptLabel: text(config.autopilot.exceptLabel),
    autopilotMaxMergeSessions: text(config.autopilot.maxMergeSessions),
    command: config.claude.command,
    permissionMode: text(config.claude.permissionMode),
    gitBashPath: text(config.claude.gitBashPath),
    allowedTools: config.claude.allowedTools.join('\n'),
    promptExtra: config.promptExtra,
  };
}

function profileOf(row: ProfileRow): ProfileDto {
  return {
    ...(row.id === null ? {} : { id: row.id }),
    name: row.name.trim(),
    model: optional(row.model),
    effort: optional(row.effort),
    maxTurns: number(row.maxTurns),
    timeoutMinutes: number(row.timeoutMinutes),
    accounts: row.accounts,
  };
}

function ruleOf(row: RuleRow): RuleDto {
  return {
    ...(row.id === null ? {} : { id: row.id }),
    when: {
      labels: row.labels.trim() === '' ? null : list(row.labels, /,/),
      priorities: row.priorities.trim() === '' ? null : list(row.priorities, /,/),
      filesMin: row.filesMin.trim() === '' ? null : number(row.filesMin),
      planMinChars: row.planMinChars.trim() === '' ? null : number(row.planMinChars),
    },
    profile: row.profile.trim(),
    accounts: row.accounts,
  };
}

// What the form does not edit (prompts, setup steps, Claude limits) stays as `base` has it.
export function toConfig(form: DispatcherForm, base: DispatcherConfig): DispatcherConfig {
  const window =
    form.from.trim() === '' && form.to.trim() === ''
      ? null
      : { from: form.from.trim(), to: form.to.trim() };
  return {
    ...base,
    enabled: form.enabled,
    wip: number(form.wip),
    window,
    optInLabel: optional(form.optInLabel),
    optOutLabel: optional(form.optOutLabel),
    maxAttempts: number(form.maxAttempts),
    pauseMinutesOnLimit: number(form.pauseMinutesOnLimit),
    promptExtra: form.promptExtra,
    mergeStrategy: form.mergeStrategy,
    reviewBeforeMerge: form.reviewBeforeMerge,
    joinWorktrees: form.joinWorktrees,
    claude: {
      ...base.claude,
      command: form.command.trim() === '' ? 'claude' : form.command.trim(),
      permissionMode: optional(form.permissionMode),
      gitBashPath: optional(form.gitBashPath),
      allowedTools: list(form.allowedTools, /\n/),
    },
    profiles: form.profiles.map(profileOf),
    defaultProfile: form.defaultProfile.trim(),
    rules: form.rules.map(ruleOf),
    escalate: form.escalate,
    ladder: form.ladder.map((step) => step.profile.trim()),
    allowFable: form.allowFable,
    research: {
      auto: form.researchAuto,
      wip: number(form.researchWip),
      maxPerDay: number(form.researchMaxPerDay),
      profile: optional(form.researchProfile),
    },
    planning: {
      auto: form.planningAuto,
      wip: number(form.planningWip),
      acceptance: form.planningAcceptance,
      profile: optional(form.planningProfile),
    },
    autopilot: {
      enabled: form.autopilotEnabled,
      exceptLabel: optional(form.autopilotExceptLabel),
      maxMergeSessions: number(form.autopilotMaxMergeSessions),
    },
    accountStrategy: form.accountStrategy,
    accountLimitPercent: number(form.accountLimitPercent),
    accountReserveFiveHour: number(form.accountReserveFiveHour),
    accountReserveWeek: number(form.accountReserveWeek),
    accountResetGraceMinutes: number(form.accountResetGraceMinutes),
  };
}

function wholeNumber(value: string, min: number, max?: number): string | null {
  const trimmed = value.trim();
  const parsed = Number(trimmed);
  const inside = Number.isInteger(parsed) && parsed >= min && (max === undefined || parsed <= max);
  if (trimmed !== '' && inside) return null;
  return max === undefined
    ? `Enter a whole number from ${String(min)}.`
    : `Enter a whole number from ${String(min)} to ${String(max)}.`;
}

// The checks the daemon repeats; the same field paths come back from it.
export function checkForm(form: DispatcherForm): Record<string, string> {
  const errors: Record<string, string> = {};
  const whole = (path: string, value: string, min: number, max?: number) => {
    const problem = wholeNumber(value, min, max);
    if (problem !== null) errors[path] = problem;
  };
  const optionalNumber = (path: string, value: string, min: number) => {
    if (value.trim() !== '') whole(path, value, min);
  };
  whole('wip', form.wip, 0);
  whole('maxAttempts', form.maxAttempts, 1);
  whole('pauseMinutesOnLimit', form.pauseMinutesOnLimit, 1);
  whole('accountLimitPercent', form.accountLimitPercent, 50, 100);
  whole('accountReserveFiveHour', form.accountReserveFiveHour, 0, 50);
  whole('accountReserveWeek', form.accountReserveWeek, 0, 50);
  whole('accountResetGraceMinutes', form.accountResetGraceMinutes, 0, 300);
  whole('research.wip', form.researchWip, 1, 5);
  whole('research.maxPerDay', form.researchMaxPerDay, 1);
  whole('planning.wip', form.planningWip, 1, 5);
  whole('autopilot.maxMergeSessions', form.autopilotMaxMergeSessions, 1, 5);
  if (form.defaultProfile.trim() === '') errors['defaultProfile'] = 'Pick a profile.';
  const oneTime = form.from.trim() === '' || form.to.trim() === '';
  if (oneTime && form.from.trim() !== form.to.trim()) {
    errors['window'] = 'Fill in both times, or leave both empty.';
  }
  const names = new Set<string>();
  form.profiles.forEach((profile, at) => {
    const name = profile.name.trim();
    if (name === '') errors[`profiles[${String(at)}].name`] = 'A profile needs a name.';
    else if (names.has(name))
      errors[`profiles[${String(at)}].name`] = 'Another profile has this name.';
    names.add(name);
    whole(`profiles[${String(at)}].maxTurns`, profile.maxTurns, 1);
    whole(`profiles[${String(at)}].timeoutMinutes`, profile.timeoutMinutes, 1);
  });
  form.rules.forEach((rule, at) => {
    const path = `rules[${String(at)}]`;
    optionalNumber(`${path}.when.filesMin`, rule.filesMin, 1);
    optionalNumber(`${path}.when.planMinChars`, rule.planMinChars, 1);
    const none =
      rule.labels.trim() === '' &&
      rule.priorities.trim() === '' &&
      rule.filesMin.trim() === '' &&
      rule.planMinChars.trim() === '';
    if (none) errors[`${path}.when`] = 'Set at least one condition.';
    if (rule.profile.trim() === '') errors[`${path}.profile`] = 'Pick a profile.';
  });
  form.ladder.forEach((step, at) => {
    if (step.profile.trim() === '') errors[`ladder[${String(at)}]`] = 'Pick a profile.';
  });
  return errors;
}

const nextKey = (rows: readonly { key: number }[]): number =>
  rows.reduce((highest, row) => Math.max(highest, row.key), -1) + 1;

// References to a profile are names; a rename or removal has to follow them everywhere.
function retarget(form: DispatcherForm, from: string, to: string): DispatcherForm {
  if (from === '') return form;
  const follow = (name: string) => (name.trim() === from ? to : name);
  return {
    ...form,
    defaultProfile: follow(form.defaultProfile),
    researchProfile: follow(form.researchProfile),
    planningProfile: follow(form.planningProfile),
    rules: form.rules.map((rule) => ({ ...rule, profile: follow(rule.profile) })),
    ladder: form.ladder.map((step) => ({ ...step, profile: follow(step.profile) })),
  };
}

export function changeProfile(
  form: DispatcherForm,
  key: number,
  patch: Partial<Omit<ProfileRow, 'key'>>,
): DispatcherForm {
  const before = form.profiles.find((profile) => profile.key === key);
  const changed = {
    ...form,
    profiles: form.profiles.map((profile) =>
      profile.key === key ? { ...profile, ...patch } : profile,
    ),
  };
  const target = patch.name?.trim() ?? '';
  if (before === undefined || target === '') return changed;
  const followed = retarget(changed, before.ref, target);
  return {
    ...followed,
    profiles: followed.profiles.map((profile) =>
      profile.key === key ? { ...profile, ref: target } : profile,
    ),
  };
}

export function addProfile(form: DispatcherForm, defaults: DispatcherConfig): DispatcherForm {
  const row: ProfileRow = {
    key: nextKey(form.profiles),
    id: null,
    name: '',
    ref: '',
    model: text(defaults.claude.model),
    effort: text(defaults.claude.effort),
    maxTurns: text(defaults.claude.maxTurns),
    timeoutMinutes: text(defaults.claude.timeoutMinutes),
    accounts: [],
  };
  return { ...form, profiles: [...form.profiles, row] };
}

// A removed profile leaves its references empty and off the ladder.
export function removeProfile(form: DispatcherForm, key: number): DispatcherForm {
  const gone = form.profiles.find((profile) => profile.key === key);
  const rest = { ...form, profiles: form.profiles.filter((profile) => profile.key !== key) };
  if (gone === undefined) return form;
  const cleared = retarget(rest, gone.ref, '');
  return { ...cleared, ladder: cleared.ladder.filter((step) => step.profile !== '') };
}

function moved<T>(items: readonly T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= items.length || to >= items.length) {
    return [...items];
  }
  const next = [...items];
  const [item] = next.splice(from, 1);
  if (item !== undefined) next.splice(to, 0, item);
  return next;
}

export function moveProfile(form: DispatcherForm, from: number, to: number): DispatcherForm {
  return { ...form, profiles: moved(form.profiles, from, to) };
}

export function moveRule(form: DispatcherForm, from: number, to: number): DispatcherForm {
  return { ...form, rules: moved(form.rules, from, to) };
}

export function moveLadderStep(form: DispatcherForm, from: number, to: number): DispatcherForm {
  return { ...form, ladder: moved(form.ladder, from, to) };
}

export function addLadderStep(form: DispatcherForm): DispatcherForm {
  const taken = new Set(form.ladder.map((step) => step.profile));
  const next = form.profiles.find(
    (profile) => profile.name.trim() !== '' && !taken.has(profile.name.trim()),
  );
  const step: LadderStep = { key: nextKey(form.ladder), profile: next?.name.trim() ?? '' };
  return { ...form, ladder: [...form.ladder, step] };
}

export function addRule(form: DispatcherForm): DispatcherForm {
  const row: RuleRow = {
    key: nextKey(form.rules),
    id: null,
    labels: '',
    priorities: '',
    filesMin: '',
    planMinChars: '',
    profile: form.defaultProfile,
    accounts: [],
  };
  return { ...form, rules: [...form.rules, row] };
}
