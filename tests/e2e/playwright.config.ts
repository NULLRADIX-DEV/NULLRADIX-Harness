import { defineConfig, devices } from '@playwright/test';

const ci = process.env['CI'] !== undefined;

export default defineConfig({
  testDir: 'specs',
  globalSetup: './support/global-setup.ts',
  timeout: 90_000,
  expect: { timeout: 15_000 },
  workers: 2,
  forbidOnly: ci,
  reporter: ci ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: { trace: 'retain-on-failure' },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
