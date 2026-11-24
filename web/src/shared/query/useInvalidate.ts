import { useQueryClient, type QueryKey } from '@tanstack/react-query';
import { useCallback } from 'react';

export function useInvalidate(prefix: QueryKey): () => Promise<void> {
  const queryClient = useQueryClient();
  return useCallback(
    () => queryClient.invalidateQueries({ queryKey: prefix }),
    [queryClient, prefix],
  );
}
