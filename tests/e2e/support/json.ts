// Narrows untrusted JSON from the daemon without casts.
export function record(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`${what} is not an object`);
  }
  return Object.fromEntries(Object.entries(value));
}

export function text(value: unknown, field: string): string {
  const found = record(value, 'the response')[field];
  if (typeof found !== 'string') throw new Error(`${field} is not a string`);
  return found;
}

export function list(value: unknown, field: string): unknown[] {
  const found = record(value, 'the response')[field];
  if (!Array.isArray(found)) throw new Error(`${field} is not a list`);
  return found;
}
