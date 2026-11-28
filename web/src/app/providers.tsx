import { QueryClientProvider, type QueryClient } from '@tanstack/react-query';
import { useState, type ReactNode } from 'react';
import { StartAnimation } from '../shared/ui/StartAnimation';
import { ThemeNotice } from '../shared/ui/ThemeNotice';
import { ThemeProvider } from '../shared/ui/ThemeProvider';
import { SlotThemeContext } from '../shared/ui/useSlot';
import { loadedInBuiltinShell } from './layout/ShellEscape';

// No theme animation in the built-in settings, the theme was left.
function LoadAnimation() {
  const [builtin] = useState(loadedInBuiltinShell);
  if (!builtin) return <StartAnimation />;
  return (
    <SlotThemeContext value={undefined}>
      <StartAnimation />
    </SlotThemeContext>
  );
}

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
        <LoadAnimation />
        <ThemeNotice />
      </ThemeProvider>
    </QueryClientProvider>
  );
}
