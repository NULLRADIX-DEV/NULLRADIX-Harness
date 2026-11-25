import { describe, expect, it } from 'vitest';
import { ApiProblemError, isProblem, unwrap, type Problem } from './errors';

const problem: Problem = {
  type: 'about:blank',
  title: 'Not Found',
  status: 404,
  detail: 'no such ticket',
  code: 'ticket-not-found',
};

function response(status: number, contentType?: string): Response {
  const headers = new Headers();
  if (contentType) headers.set('content-type', contentType);
  return new Response(null, { status, headers });
}

describe('isProblem', () => {
  it('accepts problem details', () => {
    expect(isProblem(problem)).toBe(true);
  });

  it('rejects other values', () => {
    expect(isProblem(null)).toBe(false);
    expect(isProblem('boom')).toBe(false);
    expect(isProblem({ ...problem, code: 7 })).toBe(false);
  });
});

describe('unwrap', () => {
  it('returns the data of a successful response', () => {
    expect(unwrap({ data: { ok: true }, response: response(200) })).toEqual({ ok: true });
  });

  it('throws ApiProblemError for problem details', () => {
    const result = { error: problem, response: response(404, 'application/problem+json') };
    expect(() => unwrap(result)).toThrow(ApiProblemError);
    expect(() => unwrap(result)).toThrow(
      expect.objectContaining({ status: 404, code: 'ticket-not-found', detail: 'no such ticket' }),
    );
    expect(() => unwrap(result)).toThrow(expect.objectContaining({ existing: null, similar: [] }));
  });

  it('keeps the key of an existing entity', () => {
    const duplicate = { ...problem, status: 409, code: 'ticket-duplicate', existing: 'ARC-0007' };
    expect(new ApiProblemError(duplicate).existing).toBe('ARC-0007');
    expect(new ApiProblemError(problem).existing).toBeNull();
  });

  it('throws a generic error for other failures', () => {
    const result = { error: 'Bad Gateway', response: response(502, 'text/plain') };
    expect(() => unwrap(result)).toThrow('request failed with status 502');
    expect(() => unwrap(result)).not.toThrow(ApiProblemError);
  });

  it('ignores problem bodies without the problem content type', () => {
    const result = { error: problem, response: response(404, 'application/json') };
    expect(() => unwrap(result)).not.toThrow(ApiProblemError);
  });
});
