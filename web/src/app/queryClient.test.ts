import { describe, expect, it } from 'vitest';
import { ApiProblemError } from '../shared/api/errors';
import { createQueryClient, shouldRetry } from './queryClient';

function problem(status: number): ApiProblemError {
  return new ApiProblemError({ type: 'about:blank', title: 't', status, detail: 'd', code: 'c' });
}

describe('shouldRetry', () => {
  it('never retries client problems', () => {
    expect(shouldRetry(0, problem(404))).toBe(false);
    expect(shouldRetry(0, problem(422))).toBe(false);
  });

  it('retries server problems and other errors at most twice', () => {
    expect(shouldRetry(0, problem(503))).toBe(true);
    expect(shouldRetry(1, new TypeError('offline'))).toBe(true);
    expect(shouldRetry(2, new TypeError('offline'))).toBe(false);
  });
});

describe('createQueryClient', () => {
  it('uses the retry policy by default', () => {
    expect(createQueryClient().getDefaultOptions().queries?.retry).toBe(shouldRetry);
  });

  it('accepts overrides', () => {
    expect(createQueryClient({ retry: false }).getDefaultOptions().queries?.retry).toBe(false);
  });
});
