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
import { adoptTabKey, askForTabKey, readTabKey, shareTabKey } from './shared/api/tabKey';
import { routes } from './app/routes';
import { useTabKeySync } from './app/useTabKeySync';
import { StepUpCapture } from './features/stepUp/StepUpCapture';
import { takeConfirmCode } from './features/stepUp/confirmLink';
import { loadThemeModule } from './shared/lib/loadThemeModule';
import { dismissStartOverlay } from './shared/lib/startOverlay';
import { themesQuery } from './shared/query/themes';

// before the router reads the address: the sign-in leaves the key of this tab in it
adoptTabKey();
shareTabKey();
const confirmCode = takeConfirmCode();

const container = document.getElementById('root');
if (!container) throw new Error('missing #root element');

const router = createBrowserRouter(routes);
const queryClient = createQueryClient();

function Root() {
  useTabKeySync(queryClient);
  useEffect(() => {
    dismissStartOverlay();
  }, []);
  return (
    <AppProviders queryClient={queryClient}>
      <RouterProvider router={router} />
      <StepUpCapture first={confirmCode} />
    </AppProviders>
  );
}

const start = async () => {
  // a tab opened without the fragment asks the other tabs for a key before it shows a hint
  if (readTabKey() === null) await askForTabKey();
  // the theme list also fills the cache the settings read
  const fetchThemes = () => queryClient.query({ ...themesQuery(), retry: false });
  await loadThemeModule({ fetchThemes }).catch(() => undefined);
  createRoot(container).render(
    <StrictMode>
      <Root />
    </StrictMode>,
  );
};

void start();
