import { ok, s, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod/v4';
import { runCli, validatedCwd } from '../../shared/cli-runtime.js';
import { evaluateCommonJs, installPrivateReadOnlyFiles } from '../../shared/private-vfs.js';
import { tscBundleSource, typescriptPrivateFiles } from '../../generated/typescript-assets.js';

const DEFAULT_CWD = '/workspace';
const TYPESCRIPT_VERSION = '5.9.2';
const TYPESCRIPT_ROOT = '/toolchain/typescript/node_modules/typescript';
const TSC_BUNDLED = '/toolchain/private/tsc.cjs';

const tscTool = toolDefinition('tsc', { requiresFilesystem: true })
  .version('5.9.2+golem.2')
  .doc('Type-check and compile TypeScript files and projects.')
  .annotations({ openWorld: false })
  .body((body) =>
    body
      .option('cwd', z.string(), {
        default: DEFAULT_CWD,
        doc: 'Project directory used to resolve files and configuration.',
      })
      .tail('args', z.string(), {
        separator: '--',
        verbatim: true,
        doc: 'TypeScript compiler options and input files.',
      })
      .stdout({ required: true, doc: 'Compiler output and diagnostics.' })
      .stderr({ required: true, doc: 'Compiler errors that cannot be reported normally.' })
      .returns(s.s32(), { doc: 'Process exit code; zero indicates success.' }),
  );

tscTool.implement({
  tsc: async (input, context) => {
    const cwd = validatedCwd(input.cwd);
    const home = `${cwd}/.golem-home`;
    return ok(
      await runCli(
        {
          argv: [
            'node',
            '/toolchain/typescript/node_modules/typescript/bin/tsc',
            ...input.args,
          ],
          cwd,
          environment: {
            HOME: home,
            PATH: `${cwd}/node_modules/.bin:/usr/local/bin:/usr/bin:/bin`,
          },
          directories: [home],
        },
        context,
        async () => {
          const restore = installPrivateReadOnlyFiles(TYPESCRIPT_ROOT, typescriptPrivateFiles);
          try {
            evaluateCommonJs(tscBundleSource, TSC_BUNDLED);
          } finally {
            restore();
          }
        },
      ),
    );
  },
});
