import { ApiProblemError } from './errors';

export type ProblemMessages = Readonly<Partial<Record<string, string>>>;

// Known codes get their own text, other client errors show the daemon's detail.
export function problemMessage(
  error: unknown,
  messages: ProblemMessages,
  fallback: string,
): string | null {
  if (!error) return null;
  if (!(error instanceof ApiProblemError)) return fallback;
  const known = messages[error.code];
  if (known !== undefined) return known;
  if (error.status < 500 && error.status !== 401) return error.detail;
  return fallback;
}
