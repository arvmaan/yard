import { defineConfig } from '@playwright/test'

const port = Number(process.env.YARD_WEB_PORT ?? 5173)
if (!Number.isInteger(port) || port < 1 || port > 65_535) {
  throw new Error('YARD_WEB_PORT must be an integer between 1 and 65535')
}

export default defineConfig({
  testDir: './tests',
  outputDir: './test-results',
  reporter: 'line',
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: 'retain-on-failure',
  },
  webServer: [
    {
      command: `npm run dev -- --port ${port}`,
      reuseExistingServer: !process.env.CI,
      url: `http://127.0.0.1:${port}`,
    },
  ],
})
