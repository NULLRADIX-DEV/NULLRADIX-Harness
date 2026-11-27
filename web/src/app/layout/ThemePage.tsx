import { useBoards, type PageName } from '@nullradix/theme-sdk';
import type { ReactNode } from 'react';
import { useSlotted } from '../../shared/ui/useSlotted';

// The page of the shown theme for "page:<name>", or the built-in page; a failing one is replaced.
export function ThemePage({ name, children }: { name: PageName; children: ReactNode }) {
  const pid = useBoards().current?.project ?? '';
  return useSlotted(`page:${name}`, { pid }, children).node;
}
