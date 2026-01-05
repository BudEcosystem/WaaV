import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    globals: true,
    environment: 'node',
    include: ['tests/api/**/*.test.ts'],
    testTimeout: 30000,
    retry: 1,
  },
  resolve: {
    alias: {
      '@': '/src',
    },
  },
});
