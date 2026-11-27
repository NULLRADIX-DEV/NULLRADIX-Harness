import type { ThemeDefinition, ThemeSlots } from '@nullradix/theme-sdk';
import { afterEach, describe, expect, it } from 'vitest';
import { getTheme, readTheme, registerTheme, unregisterTheme } from './themeRegistry';

const Button = () => null;

afterEach(() => {
  unregisterTheme('ocean');
});

describe('registerTheme', () => {
  it('keeps what a module registered until it is taken out', () => {
    expect(getTheme('ocean')).toBeUndefined();
    const ocean: ThemeDefinition = { id: 'ocean', slots: { Button } };
    registerTheme(ocean);
    expect(getTheme('ocean')).toBe(ocean);
    unregisterTheme('ocean');
    expect(getTheme('ocean')).toBeUndefined();
  });

  it('replaces a theme registered again under the same id', () => {
    registerTheme({ id: 'ocean', slots: { Button } });
    const again: ThemeDefinition = { id: 'ocean', slots: {} };
    registerTheme(again);
    expect(getTheme('ocean')).toBe(again);
  });
});

describe('readTheme', () => {
  it('copies the theme, so later changes of the module change nothing', () => {
    const slots: ThemeSlots = { Button };
    const read = readTheme({ id: 'ocean', slots }, 'ocean');
    slots.Button = () => null;
    slots.Card = () => null;
    expect(read).toEqual({ id: 'ocean', slots: { Button } });
  });

  it('reads every property once and keeps only own functions', () => {
    const reads = { slots: 0, id: 0, button: 0 };
    const slots = {
      get Button() {
        reads.button += 1;
        return reads.button === 1 ? Button : () => null;
      },
      Card: 'not a component',
      Panel: undefined,
    } as unknown as ThemeSlots;
    Object.setPrototypeOf(slots, { Dialog: () => null });
    const definition = {
      get id() {
        reads.id += 1;
        return reads.id === 1 ? 'ocean' : 'other';
      },
      get slots() {
        reads.slots += 1;
        return slots;
      },
    };
    const read = readTheme(definition, 'ocean');
    expect(read).toEqual({ id: 'ocean', slots: { Button } });
    expect(read?.slots.Button).toBe(Button);
    expect(reads).toEqual({ slots: 1, id: 1, button: 1 });
  });

  it('refuses what is no theme or has another id', () => {
    expect(readTheme(undefined, 'ocean')).toBeUndefined();
    expect(readTheme(null, 'ocean')).toBeUndefined();
    expect(readTheme('ocean', 'ocean')).toBeUndefined();
    expect(readTheme({ id: 'ocean' }, 'ocean')).toBeUndefined();
    expect(readTheme({ id: 'ocean', slots: null }, 'ocean')).toBeUndefined();
    expect(readTheme({ id: 'forest', slots: {} }, 'ocean')).toBeUndefined();
  });

  it('keeps the Mermaid look only when it is a function', () => {
    const mermaid = () => ({ theme: 'dark' as const });
    expect(readTheme({ id: 'ocean', slots: {}, mermaid }, 'ocean')?.mermaid).toBe(mermaid);
    expect(readTheme({ id: 'ocean', slots: {}, mermaid: 'x' }, 'ocean')).toEqual({
      id: 'ocean',
      slots: {},
    });
  });

  it('copies the settings schema and drops one with code in it', () => {
    const schema = { dark: { type: 'boolean', label: 'Dark', default: true } };
    const read = readTheme({ id: 'ocean', slots: {}, settingsSchema: schema }, 'ocean');
    schema.dark.default = false;
    expect(read?.settingsSchema).toEqual({
      dark: { type: 'boolean', label: 'Dark', default: true },
    });
    const withCode = { dark: { type: 'boolean', label: () => 'x', default: true } };
    expect(readTheme({ id: 'ocean', slots: {}, settingsSchema: withCode }, 'ocean')).toEqual({
      id: 'ocean',
      slots: {},
    });
  });
});
