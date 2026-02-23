import { defineConfig } from 'vitest/config';

//@ts-ignore
export default defineConfig({
  test: {
    include: [ "./typegen-tests/**/*.test.ts" ],
    globals: true,
    environment: 'node',
  },
});
