import type { MouseEvent, ReactNode } from 'react';
import { useHost } from './hooks.js';
import type { NavItem } from './host.js';
import type { NavIconName } from './slots.js';

/** Ready-made parts of the frame */
export const useShellParts = () => useHost('useShellParts').parts;

/** Link of a main navigation entry; `children` as a function replaces the look */
export function NavLink({
  id,
  className,
  role,
  onClick: onClickProp,
  children,
}: {
  id: NavIconName;
  className?: string;
  /** Role of the anchor, such as `menuitem` in a menu */
  role?: string;
  /** Runs on every click before the route change */
  onClick?: (event: MouseEvent<HTMLAnchorElement>) => void;
  children?: (item: NavItem) => ReactNode;
}) {
  const host = useHost('NavLink');
  const item = host.nav.find((n) => n.id === id);
  if (!item) return null;
  // Only a plain left click changes the route; modified and middle clicks stay with the browser
  const onClick = (event: MouseEvent<HTMLAnchorElement>) => {
    onClickProp?.(event);
    if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    host.navigate(item.href);
  };
  return (
    <a
      href={item.href}
      className={className}
      role={role}
      aria-current={item.active ? 'page' : undefined}
      data-active={item.active}
      onClick={onClick}
    >
      {children ? (
        children(item)
      ) : (
        <>
          <span className="nav-label">{item.label}</span>
          {item.badge ? <span className="nav-badge">{item.badge}</span> : null}
        </>
      )}
    </a>
  );
}

/** Board switcher of the host */
export const BoardSwitcher = () => <>{useHost('BoardSwitcher').parts.boardSwitcher}</>;
/** Button of the global search of the host */
export const SearchButton = () => <>{useHost('SearchButton').parts.search}</>;
/** Claude usage of the host */
export const UsageMeter = () => <>{useHost('UsageMeter').parts.usage}</>;
/** The current page without the default wrapper */
export const Outlet = () => <>{useHost('Outlet').parts.outlet}</>;
/** Dot of the live connection */
export const ConnectionStatus = () => <>{useHost('ConnectionStatus').parts.status}</>;
