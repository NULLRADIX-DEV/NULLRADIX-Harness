import type { ComponentType, MouseEvent, ReactNode } from 'react';
import type { ThemeSettingsSchema } from './settings.js';

/** Names of the navigation icons a theme can replace */
export type NavIconName =
  | 'board'
  | 'dashboard'
  | 'inbox'
  | 'sessions'
  | 'questions'
  | 'docs'
  | 'search'
  | 'guide'
  | 'settings';

/** Names of other core icons a theme can replace through the slot Icon (since SDK 1.2.0) */
export type IconName =
  | 'doc'
  | 'plan'
  | 'glossary'
  | 'adr'
  | 'file'
  | 'folder'
  | 'folderOpen'
  | 'empty'
  | 'edit'
  | 'comment';

/** Look of Mermaid diagrams (part of the Mermaid config); `themeCSS` goes into the diagram SVG */
export interface MermaidLook {
  theme?: 'base' | 'default' | 'dark' | 'neutral' | 'forest';
  themeVariables?: Record<string, string | boolean>;
  themeCSS?: string;
  fontFamily?: string;
}

/** Ready-made parts of the frame that a custom shell arranges freely */
export interface ShellParts {
  /** Button of the global search (also opens with Ctrl+K) */
  search: ReactNode;
  /** Board switcher with the connection dot */
  boardSwitcher: ReactNode;
  /** Claude usage (ring or bar) */
  usage: ReactNode;
  /** Single links of the main navigation in the default look, freely arranged */
  navItems: Partial<Record<NavIconName, ReactNode>>;
  /** Only the current page, without the default wrapper and connection notice */
  outlet: ReactNode;
  /** Connection notice (banner while the live connection is down) */
  connection: ReactNode;
  /** Dot of the live connection */
  status: ReactNode;
}

/** Props of the slot Shell: the whole layout around the pages */
export interface ShellProps {
  /** Key of the project of the current board; empty while there is none */
  pid: string;
  /** Links of the main navigation (NavLinks with icon and label) */
  nav: ReactNode;
  /** Content area with the connection notice and the current page */
  content: ReactNode;
  /** More parts of the frame */
  parts: ShellParts;
}

/** Props of the slot Progress: progress from done and all steps */
export interface ProgressProps {
  done: number;
  total: number;
  /** Accessible name, such as "Progress of the steps" */
  label: string;
  /** Caption next to the number, such as "steps done" */
  caption: string;
}

/** Props of the slot Button: the theme renders the `<button>` itself and passes the props on */
export interface ButtonProps {
  /** Weight of the button; default "secondary" */
  variant?: 'primary' | 'secondary' | 'danger' | 'ghost';
  type?: 'button' | 'submit';
  disabled?: boolean;
  onClick?: (event: MouseEvent<HTMLButtonElement>) => void;
  title?: string;
  autoFocus?: boolean;
  'aria-label'?: string;
  'aria-pressed'?: boolean;
  'aria-expanded'?: boolean;
  /** Extra classes of the core, such as "small" */
  className?: string;
  children?: ReactNode;
}

/** Props of the slot Card: the inside of a card; the outer element stays in the core */
export interface CardProps {
  title?: ReactNode;
  actions?: ReactNode;
  /** Classes of the outer element, such as "option-card recommended" */
  className?: string;
  children?: ReactNode;
}

/** Props of the slot Panel: the inside of an area; the outer element stays in the core */
export interface PanelProps {
  title?: ReactNode;
  className?: string;
  children?: ReactNode;
}

/** Props of the slot Dialog: only the visible shell; role, focus trap and Esc stay in the core */
export interface DialogProps {
  /** Id of the heading that names the dialog */
  labelledBy: string;
  role: 'dialog' | 'alertdialog';
  className?: string;
  children?: ReactNode;
}

/** Props of the slot Input: the theme renders `<input>`, or `<textarea>` when `multiline` is set */
export interface InputProps {
  value: string;
  onChange: (value: string) => void;
  id?: string;
  name?: string;
  type?: 'text' | 'search' | 'password' | 'email' | 'url' | 'number';
  placeholder?: string;
  disabled?: boolean;
  required?: boolean;
  autoFocus?: boolean;
  multiline?: boolean;
  /** Rows of a multiline field */
  rows?: number;
  'aria-label'?: string;
  'aria-describedby'?: string;
  className?: string;
}

/** Props of the slot StartAnimation: loading screen while the interface starts */
export interface StartAnimationProps {
  /** The start animation is over, the app is shown */
  onDone: () => void;
}

/** Pages a theme can replace as a whole */
export type PageName = 'dashboard';

/** Props of a page slot "page:<name>"; the theme page loads its data itself */
export interface PageProps {
  /** Key of the project of the current board; empty while there is none */
  pid: string;
}

/** Page slots: keys "page:<name>" replace a whole page */
export type PageSlots = { [N in PageName as `page:${N}`]?: ComponentType<PageProps> };

/** Places a theme can replace with its own components; a missing slot keeps the default */
export interface ThemeSlots extends PageSlots {
  Shell?: ComponentType<ShellProps>;
  NavIcon?: ComponentType<{ name: NavIconName }>;
  /** Other core icons (docs tree, empty state, text selection); since SDK 1.2.0 */
  Icon?: ComponentType<{ name: IconName }>;
  Progress?: ComponentType<ProgressProps>;
  Button?: ComponentType<ButtonProps>;
  Card?: ComponentType<CardProps>;
  Panel?: ComponentType<PanelProps>;
  Dialog?: ComponentType<DialogProps>;
  Input?: ComponentType<InputProps>;
  /** Covers the app at start; must call `onDone` and ends after 8 s at the latest */
  StartAnimation?: ComponentType<StartAnimationProps>;
}

/** A theme with its id (value of data-theme), its slots and optional settings */
export interface ThemeDefinition {
  id: string;
  slots: ThemeSlots;
  /** Own settings; shown under Settings, Appearance; values through `useThemeSettings()` */
  settingsSchema?: ThemeSettingsSchema;
  /** Look of Mermaid diagrams, called on every draw (CSS variables are set then); since 1.2.0 */
  mermaid?: () => MermaidLook;
}
