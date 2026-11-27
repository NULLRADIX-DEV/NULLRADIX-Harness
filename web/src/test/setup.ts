import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, beforeEach, vi } from 'vitest';
import { forgetUnsavedBoardChoice } from '../features/board/useBoardChoice';
import { FakeEventSource } from './eventSource';
import { mockMatchMedia } from './matchMedia';

beforeEach(() => {
  mockMatchMedia(false);
  FakeEventSource.instances = [];
  vi.stubGlobal('EventSource', FakeEventSource);
});

afterEach(() => {
  cleanup();
  forgetUnsavedBoardChoice();
  // a test in the node environment has no window
  if (typeof window !== 'undefined') window.localStorage.clear();
});
