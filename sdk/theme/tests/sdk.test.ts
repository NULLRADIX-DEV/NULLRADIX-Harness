import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { SDK_VERSION } from '../src/index.js';
import type {
  ButtonProps,
  CardProps,
  DialogProps,
  InputProps,
  PageName,
  PageProps,
  PageSlots,
  PanelProps,
  ProgressProps,
  StartAnimationProps,
  ThemeDefinition,
  ThemeSlots,
} from '../src/index.js';

describe('SDK_VERSION', () => {
  it('is a semver version', () => {
    expect(SDK_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it('is 1.3.0 (new: Claude theme in the package, manifest field claudeTheme)', () => {
    expect(SDK_VERSION).toBe('1.3.0');
  });

  it('matches the version in package.json', () => {
    const path = join(import.meta.dirname, '..', 'package.json');
    const pkg = JSON.parse(readFileSync(path, 'utf8')) as { name: string; version: string };
    expect(pkg.name).toBe('@nullradix/theme-sdk');
    expect(SDK_VERSION).toBe(pkg.version);
  });
});

describe('slot types', () => {
  it('allows a theme with a Progress slot', () => {
    const Progress = (props: ProgressProps) => `${String(props.done)}/${String(props.total)}`;
    const theme = {
      id: 'example',
      slots: { Progress: Progress as never },
    } satisfies ThemeDefinition;
    expect(theme.id).toBe('example');
  });

  it('knows the part slots Button, Card, Panel, Dialog and Input', () => {
    const Button = (props: ButtonProps) => props.children;
    const Card = (props: CardProps) => props.children;
    const Panel = (props: PanelProps) => props.children;
    const Dialog = (props: DialogProps) => props.children;
    const Input = (props: InputProps) => props.value;
    const theme = {
      id: 'parts',
      slots: {
        Button: Button as never,
        Card: Card as never,
        Panel: Panel as never,
        Dialog: Dialog as never,
        Input: Input as never,
      },
    } satisfies ThemeDefinition;
    const keys: (keyof ThemeSlots)[] = ['Button', 'Card', 'Panel', 'Dialog', 'Input'];
    expect(Object.keys(theme.slots)).toEqual(keys);
  });

  it('knows the slot StartAnimation with onDone', () => {
    const StartAnimation = (props: StartAnimationProps) => {
      props.onDone();
      return null;
    };
    const theme = { id: 'start', slots: { StartAnimation } } satisfies ThemeDefinition;
    const key: keyof ThemeSlots = 'StartAnimation';
    expect(Object.keys(theme.slots)).toEqual([key]);
  });
});

describe('page slots', () => {
  it('knows page:dashboard with pid, unknown pages are a type error', () => {
    const Dashboard = (props: PageProps) => props.pid;
    const name: PageName = 'dashboard';
    const pages: PageSlots = { [`page:${name}`]: Dashboard as never };
    const theme = {
      id: 'pages',
      slots: { 'page:dashboard': Dashboard as never },
    } satisfies ThemeDefinition;
    // @ts-expect-error only page:dashboard is a page slot
    const unknown: keyof ThemeSlots = 'page:inbox';
    expect(Object.keys(theme.slots)).toEqual(Object.keys(pages));
    expect(unknown).toBe('page:inbox');
  });
});
