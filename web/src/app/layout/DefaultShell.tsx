import type { ShellProps } from '@nullradix/theme-sdk';
import { BrandMark } from '../../shared/ui/BrandMark';

// The built-in shell: sidebar with the status dot, brand and navigation, then the content.
export function DefaultShell({ nav, content, parts }: ShellProps) {
  return (
    <>
      <aside className="shell-sidebar">
        <header className="shell-dock">
          <div className="dock-row">
            {parts.status}
            <span className="shell-brand">
              <BrandMark />
              NULLRADIX
            </span>
          </div>
        </header>
        <nav className="nav" aria-label="Main navigation">
          {nav}
        </nav>
      </aside>
      {content}
    </>
  );
}
