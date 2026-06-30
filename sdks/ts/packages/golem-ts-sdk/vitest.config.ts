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
      'golem:api/oplog@1.5.0': path.resolve(__dirname, 'types/golem_api_1_5_0_oplog.d.ts'),
      'golem:api/retry@1.5.0': path.resolve(__dirname, 'types/golem_api_1_5_0_retry.d.ts'),
      'golem:agent/common@1.5.0': path.resolve(__dirname, 'types/golem_agent_1_5_0_common.d.ts'),
      'golem:agent/host@1.5.0': path.resolve(__dirname, 'types/golem_agent_1_5_0_host.d.ts'),
      'golem:core/types@2.0.0': path.resolve(__dirname, 'types/golem_core_2_0_0_types.d.ts'),
      'golem:agent/common@2.0.0': path.resolve(__dirname, 'types/golem_agent_2_0_0_common.d.ts'),
      'golem:agent/host@2.0.0': path.resolve(__dirname, 'types/golem_agent_2_0_0_host.d.ts'),
      'golem:quota/types@1.5.0': path.resolve(__dirname, 'types/golem_quota_1_5_0_types.d.ts'),
      'golem:secrets/types@0.1.0': path.resolve(__dirname, 'types/golem_secrets_0_1_0_types.d.ts'),
      'golem:secrets/reveal@0.1.0': path.resolve(
        __dirname,
        'types/golem_secrets_0_1_0_reveal.d.ts',
      ),
      'wasi:clocks/wall-clock@0.2.3': path.resolve(
        __dirname,
        'types/wasi_clocks_0_2_3_wall_clock.d.ts',
      ),
      // Host bindings used by the fluent typed surfaces. Type-only at test time —
      // the surfaces only call them inside functions, so importing the package
      // barrel resolves without the live WASM host. (fluent-io.test.ts vi.mocks
      // these with in-memory fakes for its runtime tests.)
      'wasi:keyvalue/eventual@0.1.0': path.resolve(
        __dirname,
        'types/wasi_keyvalue_0_1_0_eventual.d.ts',
      ),
      'wasi:keyvalue/eventual-batch@0.1.0': path.resolve(
        __dirname,
        'types/wasi_keyvalue_0_1_0_eventual_batch.d.ts',
      ),
      'wasi:keyvalue/types@0.1.0': path.resolve(__dirname, 'types/wasi_keyvalue_0_1_0_types.d.ts'),
      'wasi:blobstore/blobstore': path.resolve(__dirname, 'types/wasi_blobstore_blobstore.d.ts'),
      'wasi:blobstore/container': path.resolve(__dirname, 'types/wasi_blobstore_container.d.ts'),
      'wasi:blobstore/types': path.resolve(__dirname, 'types/wasi_blobstore_types.d.ts'),
      'golem:websocket/client@1.5.0': path.resolve(
        __dirname,
        'types/golem_websocket_1_5_0_client.d.ts',
      ),
    },
  },
});
