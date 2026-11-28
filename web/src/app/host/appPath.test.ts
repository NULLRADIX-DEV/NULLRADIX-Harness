import { describe, expect, it } from 'vitest';
import { appPathOf } from './appPath';

describe('appPathOf', () => {
  it('keeps path, query and hash of a place of the app', () => {
    expect(appPathOf('/sessions')).toBe('/sessions');
    expect(appPathOf('/dashboard?range=7#top')).toBe('/dashboard?range=7#top');
    expect(appPathOf('/a/./b/../c')).toBe('/a/c');
  });

  it.each([
    ['an absolute address', 'https://evil.example/x'],
    ['a protocol-relative address', '//evil.example/x'],
    ['a backslash read as a slash', '/\\evil.example/x'],
    ['a tab inside the slashes', '/\t/evil.example/x'],
    ['a dot segment before the host', '/.//evil.example/x'],
    ['a parent segment before the host', '/..//evil.example/x'],
    ['a relative path', 'sessions'],
    ['a script address', 'javascript:alert(1)'],
    ['an empty address', ''],
  ])('refuses %s', (_name, to) => {
    expect(appPathOf(to)).toBeNull();
  });

  it('refuses what is no text', () => {
    expect(appPathOf(undefined)).toBeNull();
    expect(appPathOf({ toString: () => '/sessions' })).toBeNull();
  });
});
