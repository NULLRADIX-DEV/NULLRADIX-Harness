import { useQueryClient } from '@tanstack/react-query';
import { useEffect, useEffectEvent, type ReactNode } from 'react';
import { ChangeStreamConnection } from './connection';
import type { ChangeRegistry } from './registry';

export function ChangeStream({
  enabled,
  registry,
  onError,
  children,
}: {
  enabled: boolean;
  registry: ChangeRegistry;
  onError?: () => void;
  children: ReactNode;
}) {
  const queryClient = useQueryClient();
  const reportError = useEffectEvent(() => {
    onError?.();
  });

  useEffect(() => {
    if (!enabled) return;
    const connection = new ChangeStreamConnection({
      queryClient,
      registry,
      onError: () => {
        reportError();
      },
    });
    connection.open();
    return () => {
      connection.close();
    };
  }, [enabled, queryClient, registry]);

  return children;
}
