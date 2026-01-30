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
      'golem:rpc/types@0.2.2': path.resolve(__dirname, 'types/golem_rpc_0_2_2_types.d.ts'),
      'golem:api/host@1.3.0': path.resolve(__dirname, 'types/golem_api_1_3_0_host.d.ts'),
      'golem:agent/common': path.resolve(__dirname, 'types/golem_agent_common.d.ts'),
      'golem:agent/host': path.resolve(__dirname, 'types/golem_agent_host.d.ts'),
    },
  },
});
