import { useQuery } from '@tanstack/react-query';
import { api } from '../../shared/api/client';

export const HEALTH_REFRESH_MS = 10_000;

export function useHealth() {
  return useQuery({
    queryKey: ['health'],
    queryFn: async () => {
      const { data, response } = await api.GET('/health');
      if (!data) throw new Error(`health check failed with status ${String(response.status)}`);
      return data;
    },
    refetchInterval: HEALTH_REFRESH_MS,
    retry: false,
  });
}
