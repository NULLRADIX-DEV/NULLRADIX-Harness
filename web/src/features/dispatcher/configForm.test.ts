import { describe, expect, it } from 'vitest';
import { defaultConfig } from '../../test/dispatcherFixture';
import {
  addLadderStep,
  addProfile,
  addRule,
  changeProfile,
  checkForm,
  moveLadderStep,
  moveProfile,
  moveRule,
  removeProfile,
  toConfig,
  toForm,
  type DispatcherForm,
} from './configForm';

function form(change: (form: DispatcherForm) => DispatcherForm = (same) => same): DispatcherForm {
  return change(toForm(defaultConfig()));
}

function at<T>(items: readonly T[], index: number): T {
  const item = items[index];
  if (item === undefined) throw new Error(`no item at ${String(index)}`);
  return item;
}

describe('toForm and toConfig', () => {
  it('give back the configuration they were made from', () => {
    const config = defaultConfig();
    const next = toConfig(toForm(config), config);
    expect({ ...next, rules: [] }).toEqual({ ...config, rules: [] });
    expect(next.rules).toEqual(
      config.rules.map((rule) => ({
        ...rule,
        when: { labels: null, priorities: null, filesMin: null, planMinChars: null, ...rule.when },
      })),
    );
    expect(toConfig(toForm(next), next)).toEqual(next);
  });

  it('keep what the form does not edit', () => {
    const config = defaultConfig();
    config.prompts.work = 'own prompt';
    config.worktreeSetup = [{ cwd: '.', run: 'npm ci' }];
    config.claude.extraArgs = ['--x'];
    config.staleMinutes = 999;
    const next = toConfig({ ...toForm(config), wip: '4' }, config);
    expect(next.prompts.work).toBe('own prompt');
    expect(next.worktreeSetup).toEqual([{ cwd: '.', run: 'npm ci' }]);
    expect(next.claude.extraArgs).toEqual(['--x']);
    expect(next.staleMinutes).toBe(999);
    expect(next.wip).toBe(4);
  });

  it('read a window and lists from text', () => {
    const config = defaultConfig();
    const edited: DispatcherForm = {
      ...toForm(config),
      from: ' 18:00 ',
      to: '07:00',
      optInLabel: '  ',
      allowedTools: ' Read \n\n Edit ',
      rules: [
        {
          key: 0,
          id: null,
          labels: ' docs , typo ',
          priorities: '',
          filesMin: '3',
          planMinChars: '',
          profile: 'small',
          accounts: ['max'],
        },
      ],
    };
    const next = toConfig(edited, config);
    expect(next.window).toEqual({ from: '18:00', to: '07:00' });
    expect(next.optInLabel).toBeNull();
    expect(next.claude.allowedTools).toEqual(['Read', 'Edit']);
    expect(next.rules).toEqual([
      {
        when: { labels: ['docs', 'typo'], priorities: null, filesMin: 3, planMinChars: null },
        profile: 'small',
        accounts: ['max'],
      },
    ]);
  });
});

describe('checkForm', () => {
  it('accepts the default configuration', () => {
    expect(checkForm(form())).toEqual({});
  });

  it('names every wrong number by the path the daemon uses', () => {
    const errors = checkForm(
      form((f) => ({
        ...f,
        wip: '-1',
        maxAttempts: '0',
        pauseMinutesOnLimit: 'x',
        accountLimitPercent: '49',
        accountReserveFiveHour: '51',
        accountReserveWeek: '',
        accountResetGraceMinutes: '301',
        researchWip: '6',
        researchMaxPerDay: '0',
        planningWip: '0',
        autopilotMaxMergeSessions: '6',
      })),
    );
    expect(Object.keys(errors).sort()).toEqual(
      [
        'wip',
        'maxAttempts',
        'pauseMinutesOnLimit',
        'accountLimitPercent',
        'accountReserveFiveHour',
        'accountReserveWeek',
        'accountResetGraceMinutes',
        'research.wip',
        'research.maxPerDay',
        'planning.wip',
        'autopilot.maxMergeSessions',
      ].sort(),
    );
    expect(errors['accountLimitPercent']).toBe('Enter a whole number from 50 to 100.');
    expect(errors['wip']).toBe('Enter a whole number from 0.');
  });

  it('wants both window times or none', () => {
    expect(checkForm(form((f) => ({ ...f, from: '08:00' })))['window']).toBeDefined();
    expect(
      checkForm(form((f) => ({ ...f, from: '08:00', to: '18:00' })))['window'],
    ).toBeUndefined();
  });

  it('checks profiles, rules, ladder and the default profile', () => {
    const errors = checkForm(
      form((f) => ({
        ...f,
        defaultProfile: '',
        profiles: [
          { ...at(f.profiles, 0), name: ' ' },
          { ...at(f.profiles, 1), name: 'large', maxTurns: '0' },
          { ...at(f.profiles, 2), timeoutMinutes: 'x' },
        ],
        rules: [
          { ...at(f.rules, 0), labels: '', profile: '' },
          { ...at(f.rules, 1), filesMin: 'x' },
        ],
        ladder: [{ key: 0, profile: '' }],
      })),
    );
    expect(Object.keys(errors).sort()).toEqual(
      [
        'defaultProfile',
        'profiles[0].name',
        'profiles[1].maxTurns',
        'profiles[2].name',
        'profiles[2].timeoutMinutes',
        'rules[0].when',
        'rules[0].profile',
        'rules[1].when.filesMin',
        'ladder[0]',
      ].sort(),
    );
  });
});

describe('profile edits', () => {
  it('follow a rename through every reference', () => {
    const f = form((current) => ({
      ...current,
      researchProfile: 'large',
      planningProfile: 'large',
    }));
    const next = changeProfile(f, at(f.profiles, 2).key, { name: ' huge ' });
    expect(at(next.profiles, 2).name).toBe(' huge ');
    expect(next.rules.map((rule) => rule.profile)).toEqual(['small', 'huge', 'huge', 'huge']);
    expect(next.ladder.map((step) => step.profile)).toEqual(['small', 'medium', 'huge']);
    expect(next.researchProfile).toBe('huge');
    expect(next.planningProfile).toBe('huge');
    expect(next.defaultProfile).toBe('medium');
  });

  it('follow a rename that is typed through an empty name', () => {
    let f = form();
    const key = at(f.profiles, 2).key;
    for (const name of ['', 'h', 'hu', 'huge']) f = changeProfile(f, key, { name });
    expect(at(f.profiles, 2).ref).toBe('huge');
    expect(f.rules.map((rule) => rule.profile)).toEqual(['small', 'huge', 'huge', 'huge']);
    expect(f.ladder.map((step) => step.profile)).toEqual(['small', 'medium', 'huge']);
  });

  it('leave references alone when other fields change', () => {
    const f = form();
    const next = changeProfile(f, at(f.profiles, 1).key, { maxTurns: '99' });
    expect(at(next.profiles, 1).maxTurns).toBe('99');
    expect(next.ladder).toEqual(f.ladder);
    expect(next.defaultProfile).toBe('medium');
  });

  it('clear references and leave the ladder when a profile is removed', () => {
    const f = form();
    const next = removeProfile(f, at(f.profiles, 1).key);
    expect(next.profiles.map((profile) => profile.name)).toEqual(['small', 'large']);
    expect(next.defaultProfile).toBe('');
    expect(next.ladder.map((step) => step.profile)).toEqual(['small', 'large']);
    expect(removeProfile(f, 99)).toBe(f);
  });

  it('add a profile with the Claude defaults and a fresh key', () => {
    const next = addProfile(form(), defaultConfig());
    expect(at(next.profiles, 3)).toMatchObject({
      name: '',
      id: null,
      model: 'sonnet',
      effort: '',
      maxTurns: '80',
      timeoutMinutes: '60',
      accounts: [],
    });
    const own = defaultConfig();
    own.claude = { ...own.claude, model: 'opus', effort: 'max' };
    expect(at(addProfile(form(), own).profiles, 3)).toMatchObject({ model: 'opus', effort: 'max' });
    expect(new Set(next.profiles.map((profile) => profile.key)).size).toBe(4);
  });

  it('move profiles, ladder steps and rules within bounds', () => {
    const f = form();
    expect(moveProfile(f, 0, 1).profiles.map((profile) => profile.name)).toEqual([
      'medium',
      'small',
      'large',
    ]);
    expect(moveProfile(f, 0, -1).profiles).toEqual(f.profiles);
    expect(moveProfile(f, 2, 3).profiles).toEqual(f.profiles);
    expect(moveLadderStep(f, 2, 0).ladder.map((step) => step.profile)).toEqual([
      'large',
      'small',
      'medium',
    ]);
    expect(at(moveRule(f, 1, 0).rules, 0).profile).toBe('large');
  });
});

describe('adding steps and rules', () => {
  it('offers a profile that is not on the ladder yet', () => {
    const f = form((current) => ({ ...current, ladder: current.ladder.slice(0, 1) }));
    expect(addLadderStep(f).ladder.map((step) => step.profile)).toEqual(['small', 'medium']);
    expect(at(addLadderStep(form()).ladder, 3).profile).toBe('');
  });

  it('starts a rule on the default profile', () => {
    const next = addRule(form());
    expect(next.rules).toHaveLength(5);
    expect(at(next.rules, 4)).toMatchObject({ profile: 'medium', labels: '', accounts: [] });
  });
});

describe('row ids', () => {
  const ids = (): DispatcherForm => {
    const config = defaultConfig();
    config.profiles.forEach(
      (profile, at) => (profile.id = `00000000-0000-7000-8000-00000000001${String(at)}`),
    );
    config.rules.forEach(
      (rule, at) => (rule.id = `00000000-0000-7000-8000-00000000002${String(at)}`),
    );
    return toForm(config);
  };

  it('travel through the form, a rename and a reorder', () => {
    const f = moveRule(
      moveProfile(changeProfile(ids(), at(ids().profiles, 0).key, { name: 'tiny' }), 0, 2),
      0,
      3,
    );
    const next = toConfig(f, defaultConfig());
    expect(next.profiles.map((profile) => [profile.name, profile.id])).toEqual([
      ['medium', '00000000-0000-7000-8000-000000000011'],
      ['large', '00000000-0000-7000-8000-000000000012'],
      ['tiny', '00000000-0000-7000-8000-000000000010'],
    ]);
    expect(next.rules.map((rule) => rule.id)).toEqual([
      '00000000-0000-7000-8000-000000000021',
      '00000000-0000-7000-8000-000000000022',
      '00000000-0000-7000-8000-000000000023',
      '00000000-0000-7000-8000-000000000020',
    ]);
  });

  it('are left out for new rows', () => {
    const f = addRule(addProfile(form(), defaultConfig()));
    const next = toConfig(f, defaultConfig());
    expect('id' in at(next.profiles, 3)).toBe(false);
    expect('id' in at(next.rules, 4)).toBe(false);
  });
});
