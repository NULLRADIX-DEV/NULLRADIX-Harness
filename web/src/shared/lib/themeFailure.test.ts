import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { forgetThemeFailures, reportThemeFailure } from './themeFailure';
import { getThemeNotice, setThemeNotice } from './themeNotice';

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

afterEach(() => {
  forgetThemeFailures();
  setThemeNotice(undefined);
});

it('logs every failure and tells the person once per theme', () => {
  reportThemeFailure('ocean', 'the slot Button', new Error('one'));
  expect(getThemeNotice()?.message).toContain('"ocean"');
  setThemeNotice(undefined);
  reportThemeFailure('ocean', 'the slot Card', new Error('two'));
  reportThemeFailure('ocean', 'the slot Panel', new Error('three'));
  expect(console.error).toHaveBeenCalledTimes(3);
  expect(getThemeNotice()).toBeUndefined();
  reportThemeFailure('forest', 'the slot Button', new Error('four'));
  expect(getThemeNotice()?.message).toContain('"forest"');
});
