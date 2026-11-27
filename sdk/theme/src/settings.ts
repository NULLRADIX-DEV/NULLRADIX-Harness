/** A theme setting: a switch */
export interface ThemeSettingBoolean {
  type: 'boolean';
  label: string;
  hint?: string;
  default: boolean;
}

/** A theme setting: a choice from fixed options */
export interface ThemeSettingSelect {
  type: 'select';
  label: string;
  hint?: string;
  options: { value: string; label: string }[];
  default: string;
}

/** One setting of a theme */
export type ThemeSetting = ThemeSettingBoolean | ThemeSettingSelect;

/** Theme settings by key (`^[a-zA-Z][a-zA-Z0-9_-]{0,39}$`, at most 50) */
export type ThemeSettingsSchema = Record<string, ThemeSetting>;

/** Values for a schema: switches as boolean, choices as string */
export type ThemeSettingValues<S extends ThemeSettingsSchema> = {
  [K in keyof S]: S[K] extends ThemeSettingBoolean ? boolean : string;
};

/** Stored values of the shown theme (missing or invalid ones give the default) */
export interface ThemeSettingsState {
  values: Record<string, boolean | string>;
  loading: boolean;
  /** Saves a value; the panel and all hooks see it afterwards */
  set: (key: string, value: boolean | string) => Promise<void>;
}
