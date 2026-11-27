import type { ProgressProps } from '@nullradix/theme-sdk';
import type { ReactNode } from 'react';
import { useSlotted } from './useSlotted';

// Progress of steps: the children are the built-in look, the slot Progress may replace it.
export function Progress({
  done,
  total,
  label,
  caption,
  children,
}: ProgressProps & { children: ReactNode }) {
  return useSlotted('Progress', { done, total, label, caption }, children).node;
}
