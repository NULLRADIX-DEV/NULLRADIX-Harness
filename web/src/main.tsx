import '@fontsource/geist-sans/latin-400.css';
import '@fontsource/geist-sans/latin-500.css';
import '@fontsource/geist-sans/latin-600.css';
import '@fontsource/geist-mono/latin-400.css';
import '@fontsource/geist-mono/latin-ext-400.css';
import '@fontsource/geist-mono/latin-500.css';
import '@fontsource/geist-mono/latin-ext-500.css';
import './app/styles/variables.css';
import './app/styles/base.css';
import { StrictMode, useEffect } from 'react';
import { createRoot } from 'react-dom/client';
import { RouterProvider, createBrowserRouter } from 'react-router';
import { AppProviders } from './app/providers';
import { createQueryClient } from './app/queryClient';
import { routes } from './app/routes';
import { dismissStartOverlay } from './shared/lib/startOverlay';

const container = document.getElementById('root');
if (!container) throw new Error('missing #root element');

const router = createBrowserRouter(routes);
const queryClient = createQueryClient();

function Root() {
  useEffect(() => {
    dismissStartOverlay();
  }, []);
  return (
    <AppProviders queryClient={queryClient}>
      <RouterProvider router={router} />
    </AppProviders>
  );
}

createRoot(container).render(
  <StrictMode>
    <Root />
  </StrictMode>,
);
