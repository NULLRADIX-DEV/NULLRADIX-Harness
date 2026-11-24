import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, beforeEach } from 'vitest';
import { mockMatchMedia } from './matchMedia';

beforeEach(() => {
  mockMatchMedia(false);
});

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});
