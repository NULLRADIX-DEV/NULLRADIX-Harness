import type { QueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { onTabKey } from '../shared/api/tabKey';

// A key from another tab replaces this tab's: everything fetched with the old one is asked again.
export function useTabKeySync(queryClient: QueryClient): void {
  useEffect(
    () =>
      onTabKey(() => {
        void queryClient.invalidateQueries();
      }),
    [queryClient],
  );
}
