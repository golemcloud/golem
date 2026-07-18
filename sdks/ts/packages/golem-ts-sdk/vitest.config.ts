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
      'golem:api/host@1.5.0': path.resolve(__dirname, 'types/golem_api_1_5_0_host.d.ts'),
      'golem:api/oplog@1.5.0': path.resolve(__dirname, 'types/golem_api_1_5_0_oplog.d.ts'),
      'golem:api/retry@1.5.0': path.resolve(__dirname, 'types/golem_api_1_5_0_retry.d.ts'),
      'golem:core/types@2.0.0': path.resolve(__dirname, 'types/golem_core_2_0_0_types.d.ts'),
      'golem:agent/common@2.0.0': path.resolve(__dirname, 'types/golem_agent_2_0_0_common.d.ts'),
      'golem:agent/host@2.0.0': path.resolve(__dirname, 'types/golem_agent_2_0_0_host.d.ts'),
      'golem:quota/types@1.5.0': path.resolve(__dirname, 'types/golem_quota_1_5_0_types.d.ts'),
    },
  },
});
