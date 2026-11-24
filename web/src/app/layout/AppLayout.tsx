import { NavLink, Outlet } from 'react-router';
import { DaemonStatus } from '../../features/daemon/DaemonStatus';
import { BrandMark } from '../../shared/ui/BrandMark';
import { NAV_ITEMS } from './navigation';

export function AppLayout() {
  return (
    <>
      <aside className="shell-sidebar">
        <header className="shell-dock">
          <div className="dock-row">
            <DaemonStatus />
            <span className="shell-brand">
              <BrandMark />
              NULLRADIX
            </span>
          </div>
        </header>
        <nav className="nav" aria-label="Main navigation">
          {NAV_ITEMS.map(({ path, label, Icon }) => (
            <NavLink key={path} to={path}>
              <Icon />
              <span className="nav-label">{label}</span>
            </NavLink>
          ))}
        </nav>
      </aside>
      <div className="shell-content">
        <main className="shell-main">
          <Outlet />
        </main>
      </div>
    </>
  );
}
