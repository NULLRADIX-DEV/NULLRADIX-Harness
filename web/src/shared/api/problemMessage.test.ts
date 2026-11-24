import { describe, expect, it } from 'vitest';
import { ApiProblemError } from './errors';
import { problemMessage } from './problemMessage';

function problem(status: number, code: string) {
  return new ApiProblemError({ type: 'about:blank', title: 'x', status, code, detail: 'detail' });
}

describe('problemMessage', () => {
  const messages = { 'version-conflict': 'changed elsewhere' };

  it('is empty without an error', () => {
    expect(problemMessage(null, messages, 'failed')).toBeNull();
  });

  it('prefers the message for a known code', () => {
    expect(problemMessage(problem(409, 'version-conflict'), messages, 'failed')).toBe(
      'changed elsewhere',
    );
  });

  it('shows the detail of other client errors', () => {
    expect(problemMessage(problem(422, 'name-invalid'), messages, 'failed')).toBe('detail');
  });

  it('falls back for server, auth and network errors', () => {
    expect(problemMessage(problem(500, 'internal'), messages, 'failed')).toBe('failed');
    expect(problemMessage(problem(401, 'session-invalid'), messages, 'failed')).toBe('failed');
    expect(problemMessage(new TypeError('offline'), messages, 'failed')).toBe('failed');
  });
});
