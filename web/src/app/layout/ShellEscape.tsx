import { Link } from 'react-router';
import { safeThemeUrl } from '../../shared/lib/safeTheme';
import './shellEscape.css';

// Navigation state that shows the settings in the built-in shell, whatever the theme shell renders.
export const BUILTIN_SHELL_STATE = { builtinShell: true } as const;

export function wantsBuiltinShell(state: unknown): boolean {
  return (
    typeof state === 'object' &&
    state !== null &&
    'builtinShell' in state &&
    state.builtinShell === true
  );
}

// Plain elements outside the slot Shell: the way to the settings and to safe mode stays.
export function ShellEscape({
  assign = (url) => {
    window.location.assign(url);
  },
}: {
  assign?: (url: string) => void;
}) {
  return (
    <nav className="shell-escape" aria-label="Theme escape">
      <Link className="btn" to="/settings" state={BUILTIN_SHELL_STATE}>
        Settings
      </Link>
      <button
        type="button"
        className="btn"
        onClick={() => {
          assign(safeThemeUrl(window.location.href));
        }}
      >
        Safe mode
      </button>
    </nav>
  );
}
