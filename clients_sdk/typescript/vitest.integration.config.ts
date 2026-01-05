import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    globals: true,
    environment: 'node',
    include: ['tests/integration/**/*.test.ts', 'tests/e2e/**/*.test.ts'],
    testTimeout: 60000,
    retry: 1,
  },
  resolve: {
    alias: {
      '@': '/src',
    },
  },
});
