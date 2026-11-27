import { describe, expect, it } from 'vitest';

import type { components } from '../../shared/api/schema';
import { OUTCOME_LABELS } from './StatsCards';

describe('OUTCOME_LABELS', () => {
  it('names every outcome the server sends', () => {
    const all: components['schemas']['DispatchOutcomeDto'][] = [
      'done',
      'review',
      'human',
      'abort',
      'abort-final',
      'blocked',
      'question',
      'pause-limit',
      'pause-claude-missing',
      'setup-failed',
      'taken',
      'gone',
      'other',
      'merge-session',
      'research',
      'planned',
      'start-failed',
    ];
    for (const outcome of all) expect(OUTCOME_LABELS[outcome], outcome).toBeTruthy();
    expect(OUTCOME_LABELS['start-failed']).toBe('Never started');
  });
});
