import { expect, test as base } from '@playwright/test';
import { Daemon } from './daemon';

export { expect };

interface Fixtures {
  daemon: Daemon;
}

interface Options {
  // Environment variables the daemon of a test gets on top of the scrubbed ones.
  daemonEnv: NodeJS.ProcessEnv;
}

// Every test gets its own daemon and home, and a browser that is signed in through a handoff.
export const test = base.extend<Fixtures & Options>({
  daemonEnv: [{}, { option: true }],
  daemon: async ({ daemonEnv }, use) => {
    const daemon = await Daemon.start('review', daemonEnv);
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
    const violations: string[] = [];
    page.on('console', (message) => {
      if (message.text().includes('Content Security Policy')) violations.push(message.text());
    });
    await page.goto(await daemon.handoff());
    await page.getByRole('navigation', { name: 'Main navigation' }).waitFor();
    await use(page);
    expect(violations, 'CSP violations').toEqual([]);
  },
});
