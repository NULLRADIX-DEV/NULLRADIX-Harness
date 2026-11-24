import { useQueryClient } from '@tanstack/react-query';
import { useEffect, type ReactNode } from 'react';
import { ChangeStreamConnection } from './connection';
import type { ChangeRegistry } from './registry';

export function ChangeStream({
  enabled,
  registry,
  children,
}: {
  enabled: boolean;
  registry: ChangeRegistry;
  children: ReactNode;
}) {
  const queryClient = useQueryClient();

  useEffect(() => {
    if (!enabled) return;
    const connection = new ChangeStreamConnection({ queryClient, registry });
    connection.open();
    return () => {
      connection.close();
    };
  }, [enabled, queryClient, registry]);

  return children;
}
