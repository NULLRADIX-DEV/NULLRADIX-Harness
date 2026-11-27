import type { NavIconName } from '@nullradix/theme-sdk';
import type { ComponentType } from 'react';
import {
  BoardIcon,
  DashboardIcon,
  InboxIcon,
  QuestionsIcon,
  SessionsIcon,
  SettingsIcon,
} from '../../shared/ui/icons';

export interface NavItem {
  id: NavIconName;
  path: string;
  label: string;
  Icon: ComponentType;
}

export const NAV_ITEMS: readonly NavItem[] = [
  { id: 'board', path: '/board', label: 'Board', Icon: BoardIcon },
  { id: 'inbox', path: '/inbox', label: 'Inbox', Icon: InboxIcon },
  { id: 'dashboard', path: '/dashboard', label: 'Dashboard', Icon: DashboardIcon },
  { id: 'questions', path: '/questions', label: 'Questions', Icon: QuestionsIcon },
  { id: 'sessions', path: '/sessions', label: 'Sessions', Icon: SessionsIcon },
  { id: 'settings', path: '/settings', label: 'Settings', Icon: SettingsIcon },
];
