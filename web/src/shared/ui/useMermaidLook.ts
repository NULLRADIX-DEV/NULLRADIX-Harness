import type { MermaidLook } from '@nullradix/theme-sdk';
import { useCallback } from 'react';
import { reportThemeFailure } from '../lib/themeFailure';
import { useSlotTheme } from './useSlot';

// The look of the Mermaid diagrams, read on every draw; undefined keeps the built-in look.
export function useMermaidLook(): () => MermaidLook | undefined {
  const theme = useSlotTheme();
  return useCallback(() => {
    const look = theme?.mermaid;
    if (!theme || !look) return undefined;
    try {
      return look();
    } catch (error) {
      reportThemeFailure(theme.id, 'the Mermaid look', error);
      return undefined;
    }
  }, [theme]);
}
