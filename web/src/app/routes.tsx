import { Navigate, type RouteObject } from 'react-router';
import { BoardPage } from '../features/board/BoardPage';
import { DashboardPage } from '../features/dashboard/DashboardPage';
import { InboxPage } from '../features/inbox/InboxPage';
import { QuestionsPage } from '../features/questions/QuestionsPage';
import { SessionsPage } from '../features/sessions/SessionsPage';
import { SettingsPage } from '../features/settings/SettingsPage';
import { AppLayout } from './layout/AppLayout';

export const routes: RouteObject[] = [
  {
    path: '/',
    element: <AppLayout />,
    children: [
      { index: true, element: <Navigate to="/board" replace /> },
      { path: 'board', element: <BoardPage /> },
      { path: 'inbox', element: <InboxPage /> },
      { path: 'dashboard', element: <DashboardPage /> },
      { path: 'questions', element: <QuestionsPage /> },
      { path: 'sessions', element: <SessionsPage /> },
      { path: 'settings', element: <SettingsPage /> },
      { path: '*', element: <Navigate to="/board" replace /> },
    ],
  },
];
