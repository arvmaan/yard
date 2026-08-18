import { defineConfig } from 'vitest/config'

// Minimal unit-test setup. `mapProjection.ts` is pure arithmetic with no React
// or DOM dependency, so this deliberately avoids jsdom and any global test API:
// tests import `describe`/`it`/`expect` explicitly.
export default defineConfig({
  test: {
    environment: 'node',
    globals: false,
    include: ['src/**/*.test.ts'],
  },
})
