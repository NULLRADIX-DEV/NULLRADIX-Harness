import { render } from '@testing-library/react';
import { RouterProvider, createMemoryRouter } from 'react-router';
import { AppProviders } from '../app/providers';
import { createQueryClient } from '../app/queryClient';
import { routes } from '../app/routes';

export function renderApp(path = '/') {
  const queryClient = createQueryClient({ retry: false });
  const router = createMemoryRouter(routes, { initialEntries: [path] });
  return render(
    <AppProviders queryClient={queryClient}>
      <RouterProvider router={router} />
    </AppProviders>,
  );
}
