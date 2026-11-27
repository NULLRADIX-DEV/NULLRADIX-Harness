// A version as [major, minor, patch]; missing parts count as 0.
type Version = [number, number, number];

// A version with one to three parts, optionally with a leading "v".
const VERSION_PATTERN = /^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?$/;

// A version with a wildcard part, such as `1.x` or `1.2.*`.
const WILDCARD_PATTERN = /^v?(\d+)(?:\.(\d+))?\.[x*]$/;

const COMPARATOR_PATTERN = /^(\^|~|>=|<=|>|<|=)?(.+)$/;

function parseVersion(text: string): { version: Version; parts: number } | undefined {
  const match = VERSION_PATTERN.exec(text);
  if (!match) return undefined;
  const [, major, minor, patch] = match;
  const parts = [major, minor, patch].filter((part) => part !== undefined).length;
  return { version: [Number(major), Number(minor ?? 0), Number(patch ?? 0)], parts };
}

function compare(a: Version, b: Version): number {
  return a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
}

// `1.x` is `^1` and `1.2.x` is `~1.2`.
function withoutWildcard(text: string): string {
  const match = WILDCARD_PATTERN.exec(text);
  if (!match) return text;
  return match[2] === undefined ? `^${match[1] ?? ''}` : `~${match[1] ?? ''}.${match[2]}`;
}

// One condition such as `^1.2`, `~1.0.3`, `>=1.0.0` or `=1.1.0`; no operator means a caret.
function satisfiesComparator(version: Version, comparator: string): boolean {
  if (comparator === '*' || comparator === 'x') return true;
  const match = COMPARATOR_PATTERN.exec(withoutWildcard(comparator));
  const parsed = match?.[2] === undefined ? undefined : parseVersion(match[2]);
  if (!parsed) return false;
  const { version: target, parts } = parsed;
  const order = compare(version, target);
  switch (match?.[1]) {
    case '>=':
      return order >= 0;
    case '<=':
      return order <= 0;
    case '>':
      return order > 0;
    case '<':
      return order < 0;
    case '=':
      return order === 0;
    case '~':
      if (order < 0) return false;
      return parts === 1
        ? version[0] === target[0]
        : version[0] === target[0] && version[1] === target[1];
    default:
      if (order < 0) return false;
      // ^0.x stays in the same minor, ^0.0.x in the same patch version
      if (target[0] > 0 || parts === 1) return version[0] === target[0];
      if (target[1] > 0 || parts === 2) return version[0] === 0 && version[1] === target[1];
      return order === 0;
  }
}

// Does `version` satisfy `range`? Every condition must hold; invalid input is false.
export function satisfiesSdkRange(version: string, range: string): boolean {
  const parsed = parseVersion(version.trim());
  const comparators = range.split(/[\s,]+/).filter(Boolean);
  if (!parsed || comparators.length === 0) return false;
  return comparators.every((comparator) => satisfiesComparator(parsed.version, comparator));
}
