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
  path: string;
  label: string;
  Icon: ComponentType;
}

export const NAV_ITEMS: readonly NavItem[] = [
  { path: '/board', label: 'Board', Icon: BoardIcon },
  { path: '/inbox', label: 'Inbox', Icon: InboxIcon },
  { path: '/dashboard', label: 'Dashboard', Icon: DashboardIcon },
  { path: '/questions', label: 'Questions', Icon: QuestionsIcon },
  { path: '/sessions', label: 'Sessions', Icon: SessionsIcon },
  { path: '/settings', label: 'Settings', Icon: SettingsIcon },
];
