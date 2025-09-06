import { defineConfig } from 'vitest/config';

//@ts-ignore
export default defineConfig({
    test: {
        globals: true,
        environment: 'node'
    }
});