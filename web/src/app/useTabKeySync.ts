import type { QueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { onTabKey } from '../shared/api/tabKey';

// A new key from another tab makes everything fetched with the old key stale.
export function useTabKeySync(queryClient: QueryClient): void {
  useEffect(
    () =>
      onTabKey(() => {
        void queryClient.invalidateQueries();
      }),
    [queryClient],
  );
}
