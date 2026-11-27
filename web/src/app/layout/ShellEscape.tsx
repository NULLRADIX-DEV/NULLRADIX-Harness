import { useEffect, useRef } from 'react';
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

// Whether the page was loaded on the settings the escape link showed in the built-in shell.
export function loadedInBuiltinShell(): boolean {
  // the router keeps the state of a navigation under `usr` in the history entry
  const entry: unknown = window.history.state;
  const state = typeof entry === 'object' && entry !== null && 'usr' in entry ? entry.usr : null;
  return window.location.pathname === '/settings' && wantsBuiltinShell(state);
}

// Plain elements outside the slot Shell, in the top layer: the way to the settings and safe mode.
export function ShellEscape({
  assign = (url) => {
    window.location.assign(url);
  },
}: {
  assign?: (url: string) => void;
}) {
  const bar = useRef<HTMLElement>(null);
  useEffect(() => {
    const element = bar.current;
    // without the popover API the bar stays a fixed element with the highest z-index
    if (!element || typeof element.showPopover !== 'function') return;
    try {
      element.setAttribute('popover', 'manual');
      element.showPopover();
    } catch {
      // already open
    }
  }, []);
  return (
    <nav ref={bar} className="shell-escape" aria-label="Leave theme">
      <Link className="shell-escape-item" to="/settings" state={BUILTIN_SHELL_STATE}>
        Settings
      </Link>
      <button
        type="button"
        className="shell-escape-item"
        onClick={() => {
          assign(safeThemeUrl(window.location.href));
        }}
      >
        Safe mode
      </button>
    </nav>
  );
}
