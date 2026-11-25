import type { RunDetail, TranscriptLine } from '../features/runs/dispatchQueries';
import type { Run } from '../features/runs/queries';

export const WORK_ID = '0190a0b0-0000-7000-8000-0000000000d1';
export const MERGE_ID = '0190a0b0-0000-7000-8000-0000000000d2';
export const SHOT_ID = '0190a0b0-0000-7000-8000-0000000000e1';

export function dispatchRun(overrides: Partial<Run> = {}): Run {
  return {
    id: WORK_ID,
    label: 'ARC-0001 work',
    status: 'succeeded',
    createdAt: 1_000,
    startedAt: 1_000,
    finishedAt: 126_000,
    version: 3,
    dispatch: {
      kind: 'work',
      trigger: 'dispatcher',
      subjectKey: 'ARC-0001',
      configHash: 'abc',
      profile: {
        id: null,
        name: 'standard',
        model: 'claude-sonnet-5',
        effort: 'high',
        maxTurns: 40,
        timeoutMinutes: 30,
      },
    },
    result: {
      deniedCount: 2,
      usage: {
        costUsd: 0.43,
        inputTokens: 12_400,
        outputTokens: 3_100,
        cacheReadTokens: 80_000,
        cacheWriteTokens: 900,
        numTurns: 12,
        model: 'claude-sonnet-5',
        models: [],
      },
    },
    ...overrides,
  };
}

export function runDetail(overrides: Partial<RunDetail> = {}): RunDetail {
  return {
    ...dispatchRun(),
    args: [],
    prompt: 'Fix the login of ARC-0001',
    deniedTools: [{ tool: 'Bash', count: 2 }],
    screenshots: [{ id: SHOT_ID, mediaType: 'image/png', bytes: 10, createdAt: 2_000 }],
    ...overrides,
  };
}

export function transcriptLine(
  seq: number,
  kind: TranscriptLine['kind'],
  text: string,
  extra: Partial<TranscriptLine> = {},
): TranscriptLine {
  return { seq, kind, text, ...extra };
}
