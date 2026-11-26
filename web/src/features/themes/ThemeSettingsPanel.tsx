import { useQuery } from '@tanstack/react-query';
import { ApiProblemError } from '../../shared/api/errors';
import { problemMessage } from '../../shared/api/problemMessage';
import { resolveThemeSettings, type ThemeSettingValue } from '../../shared/lib/themeSettings';
import {
  themeSettingsQuery,
  useResetThemeSettings,
  useSaveThemeSettings,
  type Theme,
} from '../../shared/query/themes';
import { FormError } from '../../shared/ui/FormError';
import { useTheme } from '../../shared/ui/ThemeProvider';

const MESSAGES = {
  'version-conflict': 'These settings changed in the meantime. They are up to date now.',
  'theme-not-found': 'This theme is not installed any more.',
  'theme-settings-invalid': 'Some values are not valid for this theme.',
};

// The server names the refused key as the field, so each message shows at its own setting.
function fieldMessages(error: unknown): Record<string, string> {
  const found: Record<string, string> = {};
  if (error instanceof ApiProblemError) {
    for (const issue of error.fields) found[issue.field] ??= issue.message;
  }
  return found;
}

function ThemeSettingsForm({ theme }: { theme: Theme }) {
  const stored = useQuery(themeSettingsQuery(theme.id));
  const save = useSaveThemeSettings(theme.id);
  const reset = useResetThemeSettings(theme.id);
  const saved = stored.data;
  const values = resolveThemeSettings(theme.settings, saved?.values ?? {});
  const busy = !saved || save.isPending || reset.isPending;
  const fields = fieldMessages(save.error);
  const general =
    Object.keys(fields).length === 0
      ? problemMessage(save.error ?? reset.error, MESSAGES, 'Could not save the settings.')
      : null;

  const change = (key: string, value: ThemeSettingValue) => {
    if (!saved) return;
    reset.reset();
    save.mutate({ values: { ...values, [key]: value }, version: saved.version });
  };

  return (
    <section className="panel page-section" aria-labelledby="theme-settings-heading">
      <h2 id="theme-settings-heading">Settings of {theme.name}</h2>
      {stored.isError && <FormError message="Could not load the settings." />}
      <div className="theme-settings">
        {theme.settings.map((setting) => {
          const value = values[setting.key];
          const message = fields[setting.key] ?? null;
          return (
            <div key={setting.key} className="theme-setting">
              {setting.type === 'boolean' ? (
                <label className="theme-check">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={value === true}
                    onChange={(event) => {
                      change(setting.key, event.target.checked);
                    }}
                  />
                  <span>{setting.label}</span>
                </label>
              ) : (
                <label className="field">
                  <span>{setting.label}</span>
                  <select
                    disabled={busy}
                    value={typeof value === 'string' ? value : setting.default}
                    onChange={(event) => {
                      change(setting.key, event.target.value);
                    }}
                  >
                    {setting.options.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              {setting.hint && <p className="field-hint muted">{setting.hint}</p>}
              <FormError message={message} />
            </div>
          );
        })}
      </div>
      <FormError message={general} />
      <div className="form-row">
        <button
          type="button"
          className="btn"
          disabled={busy || saved.version === 0}
          onClick={() => {
            save.reset();
            reset.mutate(saved?.version ?? 0);
          }}
        >
          Reset to defaults
        </button>
      </div>
    </section>
  );
}

// Settings the chosen installed theme declares; built-in themes have none.
export function ThemeSettingsPanel() {
  const { mode, installed } = useTheme();
  const theme = installed.find((candidate) => candidate.id === mode);
  if (!theme || theme.settings.length === 0) return null;
  return <ThemeSettingsForm key={theme.id} theme={theme} />;
}
