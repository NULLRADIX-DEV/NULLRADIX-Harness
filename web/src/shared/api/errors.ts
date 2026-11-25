import type { components } from './schema';

export type Problem = components['schemas']['Problem'];
export type SimilarNote = components['schemas']['SimilarNoteDto'];
export type FieldError = components['schemas']['FieldErrorDto'];

export const PROBLEM_CONTENT_TYPE = 'application/problem+json';

export class ApiProblemError extends Error {
  readonly status: number;
  readonly code: string;
  readonly detail: string;
  readonly existing: string | null;
  readonly similar: readonly SimilarNote[];
  readonly fields: readonly FieldError[];

  constructor(problem: Problem) {
    super(problem.detail);
    this.name = 'ApiProblemError';
    this.status = problem.status;
    this.code = problem.code;
    this.detail = problem.detail;
    this.existing = problem.existing ?? null;
    this.similar = problem.similar ?? [];
    this.fields = problem.errors ?? [];
  }
}

export function isProblem(value: unknown): value is Problem {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record['status'] === 'number' &&
    typeof record['code'] === 'string' &&
    typeof record['detail'] === 'string' &&
    typeof record['title'] === 'string' &&
    typeof record['type'] === 'string'
  );
}

export interface FetchResult<T> {
  data?: T;
  error?: unknown;
  response: Response;
}

export function unwrap<T>({ data, error, response }: FetchResult<T>): T {
  if (response.ok && data !== undefined) return data;
  const contentType = response.headers.get('content-type') ?? '';
  if (contentType.startsWith(PROBLEM_CONTENT_TYPE) && isProblem(error)) {
    throw new ApiProblemError(error);
  }
  throw new Error(`request failed with status ${String(response.status)}`);
}

export function unwrapEmpty(result: FetchResult<unknown>): void {
  if (result.response.ok) return;
  unwrap(result);
}

export function isUnauthorized(error: unknown): error is ApiProblemError {
  return error instanceof ApiProblemError && error.status === 401;
}
