import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, beforeEach, vi } from 'vitest';
import { FakeEventSource } from './eventSource';
import { mockMatchMedia } from './matchMedia';

beforeEach(() => {
  mockMatchMedia(false);
  FakeEventSource.instances = [];
  vi.stubGlobal('EventSource', FakeEventSource);
});

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});
