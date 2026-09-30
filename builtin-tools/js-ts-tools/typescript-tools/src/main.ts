import { ok, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod/v4';
import {
  DEFAULT_CWD,
  DEFAULT_MAX_OUTPUT_BYTES,
  DEFAULT_REGISTRY,
  ToolExecutionResultSchema,
  TYPESCRIPT_VERSION,
} from '../../shared/contracts.js';
import { runInDirectRuntime } from '../../shared/direct-runtime.js';
import { evaluateCommonJs, installPrivateReadOnlyFiles } from '../../shared/private-vfs.js';
import { tscBundleSource, typescriptPrivateFiles } from '../../generated/typescript-assets.js';

const TYPESCRIPT_ROOT = '/toolchain/typescript/node_modules/typescript';
const TSC_BUNDLED = '/toolchain/private/tsc.cjs';

const tscTool = toolDefinition('tsc', { requiresFilesystem: true })
  .version(TYPESCRIPT_VERSION)
  .doc('Run the pinned upstream TypeScript compiler CLI with Golem compatibility adapters.')
  .annotations({ openWorld: false })
  .body((body) =>
    body
      .option('cwd', z.string(), { default: DEFAULT_CWD, doc: 'Absolute working directory.' })
      .option('registry', z.string(), {
        default: DEFAULT_REGISTRY,
        doc: 'Credential-free HTTP(S) npm registry URL.',
      })
      .option('max-output-bytes', z.number().int(), {
        default: DEFAULT_MAX_OUTPUT_BYTES,
        doc: 'Combined stdout/stderr capture limit.',
      })
      .tail('args', z.string(), {
        separator: '--',
        verbatim: true,
        doc: 'Arguments passed unchanged to the compiler.',
      })
      .returns(ToolExecutionResultSchema),
  );

tscTool.implement({
  tsc: async (input) =>
    ok(
      await runInDirectRuntime(
        {
          argv: [
            'node',
            '/toolchain/typescript/node_modules/typescript/bin/tsc',
            ...input.args,
          ],
          cwd: input.cwd,
          registry: input.registry,
          maxOutputBytes: input.maxOutputBytes,
          version: TYPESCRIPT_VERSION,
        },
        async () => {
          const restore = installPrivateReadOnlyFiles(TYPESCRIPT_ROOT, typescriptPrivateFiles);
          try {
            evaluateCommonJs(tscBundleSource, TSC_BUNDLED);
          } finally {
            restore();
          }
        },
      ),
    ),
});
