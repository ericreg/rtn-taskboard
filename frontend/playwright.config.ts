import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.pw.ts',
  outputDir: '../test-results',
  use: { baseURL: 'http://127.0.0.1:5174', viewport: { width: 1600, height: 1000 } },
  webServer: {
    command: 'npm run dev -- --port 5174',
    url: 'http://127.0.0.1:5174',
    reuseExistingServer: false,
  },
});
