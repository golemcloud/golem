import { defineConfig } from 'vitest/config';
import path from 'path';

export default defineConfig({
  test: {
    globals: true,
    environment: 'node',
    setupFiles: ['./tests/testSetup.ts'],
  },
  resolve: {
    alias: {
      'golem:core/types@1.5.0': path.resolve(__dirname, 'types/golem_core_1_5_0_types.d.ts'),
      'golem:api/host@1.5.0': path.resolve(__dirname, 'types/golem_api_1_5_0_host.d.ts'),
      'golem:agent/common@1.5.0': path.resolve(__dirname, 'types/golem_agent_1_5_0_common.d.ts'),
      'golem:agent/host@1.5.0': path.resolve(__dirname, 'types/golem_agent_1_5_0_host.d.ts'),
      'wasi:clocks/wall-clock@0.2.3': path.resolve(
        __dirname,
        'types/wasi_clocks_0_2_3_wall_clock.d.ts',
      ),
    },
  },
});
