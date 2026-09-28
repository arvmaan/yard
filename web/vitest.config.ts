import { defineConfig } from 'vitest/config'

// Minimal unit-test setup. Pure `.test.ts` modules run in node. Component
// tests are `.test.tsx` files that opt into jsdom with a
// `// @vitest-environment jsdom` comment. There is no global test API: tests
// import `describe`/`it`/`expect` explicitly.
export default defineConfig({
  test: {
    environment: 'node',
    globals: false,
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
  },
})
