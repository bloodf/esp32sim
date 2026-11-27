// End-to-end GUI tests: the real esp32sim binary serves the GUI on a fresh
// copy of the mock-firmware project (see e2e/serve.mjs); Chromium drives it.
import { defineConfig } from '@playwright/test';
import { PORT } from './e2e/paths.mjs';

export default defineConfig({
  testDir: 'e2e',
  testMatch: /.*\.spec\.mjs/,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never', outputFolder: 'playwright-report' }]] : [['list']],
  outputDir: 'test-results',
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1280, height: 800 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: 'node e2e/serve.mjs',
    url: `http://127.0.0.1:${PORT}/api/info`,
    timeout: 60_000,
    reuseExistingServer: false,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
