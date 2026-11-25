import type { DispatcherConfig, DispatcherConfigView } from '../features/dispatcher/queries';
import type { DispatcherStatus } from '../features/dispatcher/statusQueries';

// The configuration a fresh daemon starts with.
export function defaultConfig(): DispatcherConfig {
  return {
    enabled: false,
    wip: 1,
    window: null,
    optInLabel: 'auto',
    optOutLabel: 'manual',
    agentPrefix: 'worker-',
    staleMinutes: 150,
    maxAttempts: 2,
    pauseMinutesOnLimit: 60,
    promptExtra: '',
    prompts: { work: null, plan: null, research: null, merge: null },
    mergeStrategy: 'merge',
    reviewBeforeMerge: true,
    joinWorktrees: 'none',
    worktreeSetup: [],
    worktreeSetupTimeoutMinutes: 15,
    claude: {
      command: 'claude',
      model: 'sonnet',
      effort: null,
      maxTurns: 80,
      timeoutMinutes: 60,
      permissionMode: 'acceptEdits',
      allowedTools: ['Read', 'Edit', 'Write'],
      useSubscription: true,
      extraArgs: [],
      gitBashPath: null,
    },
    profiles: [
      {
        name: 'small',
        model: 'sonnet',
        effort: 'high',
        maxTurns: 40,
        timeoutMinutes: 45,
        accounts: [],
      },
      {
        name: 'medium',
        model: 'opus',
        effort: 'high',
        maxTurns: 80,
        timeoutMinutes: 90,
        accounts: [],
      },
      {
        name: 'large',
        model: 'opus',
        effort: 'max',
        maxTurns: 150,
        timeoutMinutes: 120,
        accounts: [],
      },
    ],
    defaultProfile: 'medium',
    rules: [
      { when: { labels: ['docs', 'typo', 'small'] }, profile: 'small', accounts: [] },
      { when: { labels: ['architecture', 'refactoring'] }, profile: 'large', accounts: [] },
      { when: { priorities: ['urgent'] }, profile: 'large', accounts: [] },
      { when: { filesMin: 6 }, profile: 'large', accounts: [] },
    ],
    escalate: true,
    ladder: ['small', 'medium', 'large'],
    allowFable: false,
    research: { auto: false, wip: 1, maxPerDay: 10, profile: null },
    planning: { auto: false, wip: 1, acceptance: true, profile: null },
    autopilot: { enabled: false, exceptLabel: 'interactive', maxMergeSessions: 3 },
    accountStrategy: 'most-free',
    accountLimitPercent: 90,
    accountReserveFiveHour: 10,
    accountReserveWeek: 2,
    accountResetGraceMinutes: 30,
  };
}

export function configView(
  config: DispatcherConfig = defaultConfig(),
  version = 1,
): DispatcherConfigView {
  return { config, hash: '0123456789abcdef', updatedAt: 1, version };
}

export function statusDto(overrides: Partial<DispatcherStatus> = {}): DispatcherStatus {
  return {
    enabled: true,
    state: 'idle',
    message: 'Nothing is ready.',
    pausedUntil: null,
    wip: 2,
    active: 0,
    planning: 0,
    runs: [],
    tickets: [],
    lastPassAt: null,
    lastErrors: [],
    nextPassAt: null,
    agentPrefix: 'worker-',
    boardId: null,
    version: 1,
    ...overrides,
  };
}
