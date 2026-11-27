import type { StartAnimationProps } from '@nullradix/theme-sdk';
import { useCallback, useEffect, useLayoutEffect, useState } from 'react';
import { START_OVERLAY_ID } from '../lib/startOverlay';
import { SlotBoundary } from './SlotBoundary';
import { useSlotTheme } from './useSlot';
import './startAnimation.css';

// A theme animation ends after this at the latest, with or without onDone.
export const START_ANIMATION_MAX_MS = 8000;

function prefersReducedMotion(): boolean {
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

// The slot StartAnimation of the theme shown at page load covers the app; a later change adds none.
export function StartAnimation() {
  const theme = useSlotTheme();
  const [start] = useState(() => {
    const Custom = prefersReducedMotion() ? undefined : theme?.slots.StartAnimation;
    return theme && Custom ? { themeId: theme.id, Custom } : null;
  });
  const [done, setDone] = useState(false);
  const finish = useCallback(() => {
    setDone(true);
  }, []);

  useEffect(() => {
    if (!start) return undefined;
    const timer = setTimeout(finish, START_ANIMATION_MAX_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [start, finish]);

  if (!start || done) return null;
  const { themeId, Custom } = start;
  return (
    <div className="theme-start-anim" aria-hidden="true" inert>
      <SlotBoundary
        fallback={<Finish onFinish={finish} />}
        themeId={themeId}
        where="the slot StartAnimation"
      >
        <Custom onDone={finish} />
        <TakeOver />
      </SlotBoundary>
    </div>
  );
}

// Removes the built-in overlay; it commits only if the theme component renders without error.
function TakeOver() {
  useLayoutEffect(() => {
    document.getElementById(START_OVERLAY_ID)?.remove();
  }, []);
  return null;
}

// After a failure the start phase ends; the built-in overlay, if still there, fades out on its own.
function Finish({ onFinish }: { onFinish: StartAnimationProps['onDone'] }) {
  useEffect(() => {
    onFinish();
  }, [onFinish]);
  return null;
}
