import { useBoards, type ShellParts, type ShellProps } from '@nullradix/theme-sdk';
import { Fragment, useState, type ReactNode } from 'react';
import { NavLink, Outlet, useLocation } from 'react-router';
import { UsageMeter } from '../../features/accounts/UsageMeter';
import { BoardSwitcher } from '../../features/board/BoardSwitcher';
import { DaemonStatus } from '../../features/daemon/DaemonStatus';
import { NavIcon } from '../../shared/ui/ThemeIcon';
import { SlotThemeContext } from '../../shared/ui/useSlot';
import { useSlotted } from '../../shared/ui/useSlotted';
import { NullradixThemeHost } from '../host/ThemeHost';
import { DefaultShell } from './DefaultShell';
import { NAV_ITEMS } from './navigation';
import { ShellEscape, wantsBuiltinShell } from './ShellEscape';

function buildParts(): ShellParts {
  const navItems: ShellParts['navItems'] = {};
  for (const { id, path, label, Icon } of NAV_ITEMS) {
    navItems[id] = (
      <NavLink to={path}>
        <NavIcon name={id}>
          <Icon />
        </NavIcon>
        <span className="nav-label">{label}</span>
      </NavLink>
    );
  }
  // one object serves the theme and the built-in shell, so neither can change it for the other
  return Object.freeze({
    // the app has no global search or connection banner yet
    search: null,
    boardSwitcher: <BoardSwitcher />,
    usage: <UsageMeter />,
    navItems: Object.freeze(navItems),
    outlet: <Outlet />,
    connection: null,
    status: <DaemonStatus />,
  });
}

function navigationOf(parts: ShellParts): ReactNode {
  return NAV_ITEMS.map(({ id }) => <Fragment key={id}>{parts.navItems[id]}</Fragment>);
}

// The shell of the shown theme, or the built-in one; a failing theme shell is replaced.
function Shell({ parts }: { parts: ShellParts }) {
  const pid = useBoards().current?.project ?? '';
  const location = useLocation();
  const props: ShellProps = {
    pid,
    nav: navigationOf(parts),
    content: (
      <div className="shell-content">
        <main className="shell-main">
          {parts.connection}
          {parts.outlet}
        </main>
      </div>
    ),
    parts,
  };
  const standard = <DefaultShell {...props} />;
  const { node, replaced } = useSlotted('Shell', props, standard);
  // the escape link shows the settings in the built-in shell, with no theme component running
  if (location.pathname === '/settings' && wantsBuiltinShell(location.state)) {
    return <SlotThemeContext value={undefined}>{standard}</SlotThemeContext>;
  }
  return (
    <>
      {replaced && <ShellEscape />}
      {node}
    </>
  );
}

export function AppLayout() {
  const [parts] = useState(buildParts);
  return (
    <NullradixThemeHost parts={parts}>
      <Shell parts={parts} />
    </NullradixThemeHost>
  );
}
