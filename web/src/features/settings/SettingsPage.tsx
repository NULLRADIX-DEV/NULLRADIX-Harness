import { THEME_LABELS, THEME_MODES, isThemeChoice } from '../../shared/lib/theme';
import { useTheme } from '../../shared/ui/ThemeProvider';
import { AutostartSection } from './AutostartSection';
import { AccountsSection } from '../accounts/AccountsSection';
import { DispatcherSection } from '../dispatcher/DispatcherSection';
import { ProjectsSection } from '../projects/ProjectsSection';
import { CustomizationsSection } from '../customizations/CustomizationsSection';
import { SnippetsSection } from '../snippets/SnippetsSection';
import { ThemeSettingsPanel } from '../themes/ThemeSettingsPanel';
import { CatalogSection } from '../themes/CatalogSection';
import { ClaudeThemeSection } from '../themes/ClaudeThemeSection';
import { ThemesSection } from '../themes/ThemesSection';
import { Panel } from '../../shared/ui/Panel';

export function SettingsPage() {
  const { mode, setMode, installed } = useTheme();
  return (
    <div className="page narrow">
      <h1 className="page-title">Settings</h1>
      <ProjectsSection />
      <AccountsSection />
      <DispatcherSection />
      <Panel as="section" className="panel page-section">
        <h2>Appearance</h2>
        <label className="field">
          <span>Theme</span>
          <select
            value={mode}
            onChange={(event) => {
              if (isThemeChoice(event.target.value)) setMode(event.target.value);
            }}
          >
            {THEME_MODES.map((theme) => (
              <option key={theme} value={theme}>
                {THEME_LABELS[theme]}
              </option>
            ))}
            {installed.map((theme) => (
              <option key={theme.id} value={theme.id}>
                {theme.name}
              </option>
            ))}
          </select>
        </label>
      </Panel>
      <AutostartSection />
      <ThemesSection />
      <CatalogSection />
      <ThemeSettingsPanel />
      <ClaudeThemeSection />
      <SnippetsSection />
      <CustomizationsSection />
    </div>
  );
}
