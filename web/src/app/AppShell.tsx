import { useQuery, useQueryClient } from '@tanstack/react-query';
import { SessionRequired } from '../features/session/SessionRequired';
import { recheckSession, sessionQuery } from '../features/session/queries';
import { isUnauthorized } from '../shared/api/errors';
import { ChangeStream } from '../shared/live/ChangeStream';
import { AppLayout } from './layout/AppLayout';
import { liveRegistry } from './liveRegistry';

export function AppShell() {
  const queryClient = useQueryClient();
  const session = useQuery(sessionQuery());
  if (isUnauthorized(session.error)) return <SessionRequired />;
  return (
    <ChangeStream
      enabled={session.isSuccess}
      registry={liveRegistry}
      // EventSource hides the status, so an expired session shows up as a stream error
      onError={() => void recheckSession(queryClient)}
    >
      <AppLayout />
    </ChangeStream>
  );
}
