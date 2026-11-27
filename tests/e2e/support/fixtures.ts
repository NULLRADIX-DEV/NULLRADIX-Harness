import { test as base } from '@playwright/test';
import { Daemon } from './daemon';

export { expect } from '@playwright/test';

interface Fixtures {
  daemon: Daemon;
}

// Every test gets its own daemon and home, and a browser that is signed in through a handoff.
export const test = base.extend<Fixtures>({
  // eslint-disable-next-line no-empty-pattern
  daemon: async ({}, use) => {
    const daemon = await Daemon.start();
    try {
      await use(daemon);
    } finally {
      await daemon.stop();
    }
  },
  baseURL: async ({ daemon }, use) => {
    await use(daemon.baseUrl);
  },
  page: async ({ page, daemon }, use) => {
    await page.goto(await daemon.handoff());
    await page.getByRole('navigation', { name: 'Main navigation' }).waitFor();
    await use(page);
  },
});
