import type { ThemeDefinition, ThemeSlots } from '@nullradix/theme-sdk';
import { afterEach, expect, it } from 'vitest';
import { getTheme, registerTheme, unregisterTheme } from './themeRegistry';

const Button = () => null;
const ocean: ThemeDefinition = { id: 'ocean', slots: { Button } };

afterEach(() => {
  unregisterTheme('ocean');
});

it('keeps what a module registered until it is taken out', () => {
  expect(getTheme('ocean')).toBeUndefined();
  registerTheme(ocean);
  expect(getTheme('ocean')?.id).toBe('ocean');
  expect(getTheme('ocean')?.slots.Button).toBe(Button);
  unregisterTheme('ocean');
  expect(getTheme('ocean')).toBeUndefined();
});

it('replaces a theme registered again under the same id', () => {
  const Other = () => null;
  registerTheme(ocean);
  registerTheme({ id: 'ocean', slots: { Card: Other } });
  expect(getTheme('ocean')?.slots).toEqual({ Card: Other });
});

it('keeps a copy, so later changes of the theme change nothing', () => {
  const slots: ThemeSlots = { Button };
  registerTheme({ id: 'ocean', slots });
  slots.Button = () => null;
  slots.Card = () => null;
  expect(getTheme('ocean')?.slots).toEqual({ Button });
});

it('reads a getter once and keeps only own functions', () => {
  let reads = 0;
  const slots = {
    get Button() {
      reads += 1;
      return reads === 1 ? Button : () => null;
    },
    Card: 'not a component',
    Panel: undefined,
  } as unknown as ThemeSlots;
  Object.setPrototypeOf(slots, { Dialog: () => null });
  registerTheme({ id: 'ocean', slots });
  expect(getTheme('ocean')?.slots).toEqual({ Button });
  expect(getTheme('ocean')?.slots.Button).toBe(Button);
  expect(reads).toBe(1);
});

it('registers under the given id, whatever the definition says later', () => {
  let reads = 0;
  const definition = {
    slots: {},
    get id() {
      reads += 1;
      return reads === 1 ? 'ocean' : 'other';
    },
  } as ThemeDefinition;
  registerTheme(definition, 'ocean');
  expect(getTheme('ocean')?.id).toBe('ocean');
  expect(getTheme('other')).toBeUndefined();
});

it('keeps the Mermaid look and the settings schema only when they are real', () => {
  const mermaid = () => ({ theme: 'dark' as const });
  registerTheme({ id: 'ocean', slots: {}, mermaid, settingsSchema: {} });
  expect(getTheme('ocean')?.mermaid).toBe(mermaid);
  expect(getTheme('ocean')?.settingsSchema).toEqual({});
  registerTheme({ id: 'ocean', slots: {}, mermaid: 'x' as unknown as () => never });
  expect(getTheme('ocean')).toEqual({ id: 'ocean', slots: {} });
});
