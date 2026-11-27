import { QueryClientProvider, type QueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { ThemeNotice } from '../shared/ui/ThemeNotice';
import { ThemeProvider } from '../shared/ui/ThemeProvider';

export function AppProviders({
  queryClient,
  children,
}: {
  queryClient: QueryClient;
  children: ReactNode;
}) {
  return (
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>
        {children}
        <ThemeNotice />
      </ThemeProvider>
    </QueryClientProvider>
  );
}
