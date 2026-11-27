import type { UsageState, UsageWindow } from '@nullradix/theme-sdk';
import { useQuery } from '@tanstack/react-query';
import { useMemo } from 'react';
import { usageQuery, type AccountStatus } from '../../features/accounts/queries';

const NO_USAGE: UsageState = {
  enabled: false,
  available: false,
  reason: null,
  fiveHour: null,
  sevenDay: null,
};

type Window = AccountStatus['usage']['fiveHour'];

function windowOf(found: Window): UsageWindow | null {
  if (!found) return null;
  const reset = found.resetsAt == null ? null : new Date(found.resetsAt);
  const resetsAt = reset && !Number.isNaN(reset.getTime()) ? reset.toISOString() : null;
  return { percent: found.percent, resetsAt };
}

// The limits of the first account, read on the schedule of the accounts page.
export function useHostUsage(active: boolean): UsageState {
  const statuses = useQuery({ ...usageQuery(), enabled: active });
  return useMemo(() => {
    const first = statuses.data?.[0];
    if (!first) return NO_USAGE;
    const { available, reason, fiveHour, sevenDay } = first.usage;
    return {
      enabled: true,
      available,
      reason: reason ?? null,
      fiveHour: windowOf(fiveHour),
      sevenDay: windowOf(sevenDay),
    };
  }, [statuses.data]);
}
