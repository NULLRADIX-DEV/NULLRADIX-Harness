import { SDK_VERSION } from '@nullradix/theme-sdk';
import { describe, expect, it } from 'vitest';
import { satisfiesSdkRange } from './sdkRange';

describe('satisfiesSdkRange', () => {
  it.each([
    ['0.2.0', '^0.2', true],
    ['0.2.0', '^0.2.1', false],
    ['0.2.0', '^0.3', false],
    ['0.2.0', '^1.0', false],
    ['0.2.0', '~0.2.0', true],
    ['0.2.0', '0.2.0', true],
    ['0.2.0', '>=0.1.0', true],
    ['0.2.0', '*', true],
    ['0.2.0', 'nonsense', false],
    ['0.2.0', '', false],
    ['1.5.0', '^1.2', true],
    ['1.5.0', '1.5.0', true],
    ['1.5.0', '^2', false],
    ['1.5.0', '~1.4', false],
    ['1.5.0', '>=1.0.0 <2.0.0', true],
    ['1.5.0', '>1.5.0', false],
    ['1.5.0', '<=1.5', true],
    ['0.0.3', '^0.0.3', true],
    ['0.0.4', '^0.0.3', false],
  ])('%s satisfies "%s": %s', (version, range, expected) => {
    expect(satisfiesSdkRange(version, range)).toBe(expected);
  });

  it('reads a version without an operator as a caret, like the daemon', () => {
    expect(satisfiesSdkRange('1.5.0', '1.3')).toBe(true);
    expect(satisfiesSdkRange('1.2.0', '1.3')).toBe(false);
    expect(satisfiesSdkRange('2.0.0', '1.3')).toBe(false);
    expect(satisfiesSdkRange('1.5.1', '=1.5.0')).toBe(false);
    expect(satisfiesSdkRange('1.5.0', '=1.5.0')).toBe(true);
  });

  it('takes commas and spaces between conditions, and wildcards', () => {
    expect(satisfiesSdkRange('1.5.0', '>=1.0, <2')).toBe(true);
    expect(satisfiesSdkRange('2.0.0', '>=1.0, <2')).toBe(false);
    expect(satisfiesSdkRange('1.5.0', '1.x')).toBe(true);
    expect(satisfiesSdkRange('2.0.0', '1.x')).toBe(false);
    expect(satisfiesSdkRange('1.5.2', '1.5.*')).toBe(true);
    expect(satisfiesSdkRange('1.6.0', '1.5.*')).toBe(false);
  });

  it('refuses an invalid version of the host', () => {
    expect(satisfiesSdkRange('', '^1')).toBe(false);
    expect(satisfiesSdkRange('abc', '^1')).toBe(false);
  });

  it('accepts the SDK version of this build for the ranges the shipped themes use', () => {
    expect(satisfiesSdkRange(SDK_VERSION, '^1.3')).toBe(true);
    expect(satisfiesSdkRange(SDK_VERSION, '^1.0')).toBe(true);
    expect(satisfiesSdkRange(SDK_VERSION, '^9.0')).toBe(false);
  });
});
