import { Navigate, type RouteObject } from 'react-router';
import { BoardPage } from '../features/board/BoardPage';
import { DashboardPage } from '../features/dashboard/DashboardPage';
import { InboxPage } from '../features/inbox/InboxPage';
import { QuestionsPage } from '../features/questions/QuestionsPage';
import { SessionDetailPage } from '../features/sessions/SessionDetailPage';
import { SessionsPage } from '../features/sessions/SessionsPage';
import { SettingsPage } from '../features/settings/SettingsPage';
import { AppShell } from './AppShell';

export const routes: RouteObject[] = [
  {
    path: '/',
    element: <AppShell />,
    children: [
      { index: true, element: <Navigate to="/board" replace /> },
      { path: 'board', element: <BoardPage /> },
      { path: 'inbox', element: <InboxPage /> },
      { path: 'dashboard', element: <DashboardPage /> },
      { path: 'questions', element: <QuestionsPage /> },
      { path: 'sessions', element: <SessionsPage /> },
      { path: 'sessions/:sessionId', element: <SessionDetailPage /> },
      { path: 'settings', element: <SettingsPage /> },
      { path: '*', element: <Navigate to="/board" replace /> },
    ],
  },
];
