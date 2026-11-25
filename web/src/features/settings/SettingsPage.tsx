import { THEME_LABELS, THEME_MODES, isThemeMode } from '../../shared/lib/theme';
import { useTheme } from '../../shared/ui/ThemeProvider';
import { AccountsSection } from '../accounts/AccountsSection';
import { ProjectsSection } from '../projects/ProjectsSection';

export function SettingsPage() {
  const { mode, setMode } = useTheme();
  return (
    <div className="page narrow">
      <h1 className="page-title">Settings</h1>
      <ProjectsSection />
      <AccountsSection />
      <section className="panel page-section">
        <h2>Appearance</h2>
        <label className="field">
          <span>Theme</span>
          <select
            value={mode}
            onChange={(event) => {
              if (isThemeMode(event.target.value)) setMode(event.target.value);
            }}
          >
            {THEME_MODES.map((theme) => (
              <option key={theme} value={theme}>
                {THEME_LABELS[theme]}
              </option>
            ))}
          </select>
        </label>
      </section>
    </div>
  );
}
