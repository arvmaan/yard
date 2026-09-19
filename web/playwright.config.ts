import { defineConfig } from '@playwright/test'

const port = Number(process.env.YARD_WEB_PORT ?? 5173)
const fixturePort = Number(process.env.YARD_LENS_FIXTURE_PORT ?? port + 10_000)
if (!Number.isInteger(port) || port < 1 || port > 65_535) {
  throw new Error('YARD_WEB_PORT must be an integer between 1 and 65535')
}
if (!Number.isInteger(fixturePort) || fixturePort < 1 || fixturePort > 65_535) {
  throw new Error('YARD_LENS_FIXTURE_PORT must be an integer between 1 and 65535')
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
      command: `YARD_LENS_FIXTURE_ADDR=127.0.0.1:${fixturePort} cargo test --manifest-path ../Cargo.toml -p yard-server --lib http::tests::serves_playwright_runtime_lens_fixture -- --ignored --exact --nocapture`,
      reuseExistingServer: false,
      timeout: 120_000,
      url: `http://127.0.0.1:${fixturePort}/health`,
    },
    {
      command: `YARD_API_TARGET=http://127.0.0.1:${fixturePort} npm run dev -- --port ${port}`,
      reuseExistingServer: false,
      url: `http://127.0.0.1:${port}`,
    },
  ],
})
