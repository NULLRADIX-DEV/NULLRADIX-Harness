import type { ThemeSettingsState } from './settings.js';
import type { NavIconName, ShellParts } from './slots.js';

/** Entry of the main navigation */
export interface NavItem {
  id: NavIconName;
  label: string;
  /** Route target, such as `/p/<pid>/board` */
  href: string;
  /** Does the open page show this target? */
  active: boolean;
  /** Counter on the entry, such as open tasks */
  badge?: number;
}

/** A board with its project, or a project without a board */
export interface BoardRef {
  project: string;
  board: number | null;
}

/** Projects and boards of the board database with the current choice */
export interface BoardsState {
  current: BoardRef | null;
  projects: { id: string; name: string }[];
  boards: { id: number; project: string; name: string }[];
  loading: boolean;
  /** Chooses a board or project, like the board switcher of the sidebar */
  choose: (ref: BoardRef) => void;
}

/** Load of one limit window; `resetsAt` is an ISO instant */
export interface UsageWindow {
  percent: number;
  resetsAt: string | null;
}

/** Claude usage of the first active account; `reason` explains missing values */
export interface UsageState {
  enabled: boolean;
  available: boolean;
  reason: string | null;
  fiveHour: UsageWindow | null;
  sevenDay: UsageWindow | null;
}

/** User settings a theme may read and change */
export interface ThemeSettings {
  theme: string;
  claudeUsage: boolean;
  compactCards: boolean;
  heroImages: boolean;
}

/** User settings; `values` is null while they load */
export interface SettingsState {
  values: ThemeSettings | null;
  loading: boolean;
  update: (patch: Partial<ThemeSettings>) => Promise<void>;
}

/** Shown color scheme: light or dark, theme id (data-theme) and setting (data-theme-mode) */
export interface ColorSchemeState {
  scheme: 'light' | 'dark';
  themeId: string;
  mode: string;
}

/** Contract between the NULLRADIX host and a theme: everything the SDK hooks return */
export interface ThemeHost {
  nav: NavItem[];
  boards: BoardsState;
  usage: UsageState;
  settings: SettingsState;
  /** Settings of the shown theme (`settingsSchema`) */
  themeSettings: ThemeSettingsState;
  colorScheme: ColorSchemeState;
  navigate: (to: string) => void;
  /** Opens the global search of the host */
  openSearch: () => void;
  /** Ready-made parts of the frame (like `ShellProps.parts`) */
  parts: ShellParts;
}
