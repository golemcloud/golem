import { existsSync, lstatSync, readFileSync, readdirSync, rmdirSync, unlinkSync } from 'node:fs';
import fsPromises from 'node:fs/promises';
import { createRequire, stripTypeScriptTypes } from 'node:module';
import { join, resolve } from 'node:path';
import { ok, processExitStatusSchema, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod/v4';
import { runCli, validatedCwd } from '../../shared/cli-runtime.js';
import { evaluateCommonJs, installPrivateReadOnlyFiles } from '../../shared/private-vfs.js';
import { npmBundleSource, npmPrivateFiles } from '../../generated/npm-assets.js';
import {
  DEFAULT_REGISTRY,
  installNpmRecursiveRmPatch,
  installNpmTransportWarningFilter,
  NPM_VERSION,
  npmInvocation,
} from './npm-runtime.js';
import { compileEvalSource, NodeOptionError, parseNodeArguments } from './node-args.js';
import type { NodeInvocation } from './node-args.js';
import { rewriteNpxArguments } from './npx-args.js';
import { npxTypeScriptShim } from './npx-typescript-shim.js';

const DEFAULT_CWD = '/';
const NPM_ROOT = '/toolchain/npm/node_modules/npm';
const NPM_BUNDLED = '/toolchain/private/npm-cli.cjs';

type NpmBundle = {
  default: (process: NodeJS.Process) => Promise<unknown>;
  definitions: Record<string, { type?: unknown }>;
  shorthands: Record<string, string[]>;
};

const nodeTool = toolDefinition('node', { requiresFilesystem: true })
  .version('0.1.9')
  .doc(
    "Run a JavaScript or TypeScript file (.js, .mjs, .cjs, .ts, .mts, .cts) or inline source with Golem's Node-compatible runtime. TypeScript runs without flags: types are removed and not checked; use the tsc tool to type-check.",
  )
  .annotations({ openWorld: false })
  .body((body) =>
    body
      .option('cwd', z.string(), {
        default: DEFAULT_CWD,
        env: 'PWD',
        doc: 'Directory used to resolve files and relative paths. A shell supplies its current directory.',
      })
      .tail('args', z.string(), {
        separator: '--',
        verbatim: true,
        doc: 'A script path and its arguments, or -e/--eval with JavaScript or TypeScript source, or -v/--version. Before the script or -e, --input-type=commonjs or commonjs-typescript selects how -e source is read; --input-type=module is not supported, so run a .mjs or .mts file instead. --experimental-strip-types, --experimental-transform-types, --no-experimental-strip-types, --no-warnings and --enable-source-maps are accepted and change nothing. Other Node options are rejected.',
      })
      .stdout({
        required: true,
        doc: 'Output written by the JavaScript program.',
      })
      .stderr({
        required: true,
        doc: 'Diagnostics written by the JavaScript program.',
      })
      .returns(processExitStatusSchema, {
        doc: 'Process exit status; exit_code zero indicates success.',
      }),
  );

const npmTool = toolDefinition('npm', { requiresFilesystem: true })
  .version('10.9.9+golem.9')
  .doc('Install and manage JavaScript packages and run package scripts with npm.')
  .annotations({ openWorld: false })
  .body((body) =>
    body
      .option('cwd', z.string(), {
        default: DEFAULT_CWD,
        env: 'PWD',
        doc: 'Project directory in which npm runs. A shell supplies its current directory.',
      })
      .option('registry', z.string(), {
        default: DEFAULT_REGISTRY,
        doc: 'Package registry used to resolve dependencies.',
      })
      .tail('args', z.string(), {
        separator: '--',
        verbatim: true,
        doc: 'An npm command followed by its arguments.',
      })
      .stdout({ required: true, doc: 'Normal npm command output.' })
      .stderr({ required: true, doc: 'npm warnings and diagnostics.' })
      .returns(processExitStatusSchema, {
        doc: 'Process exit status; exit_code zero indicates success.',
      }),
  );

const npxTool = toolDefinition('npx', { requiresFilesystem: true })
  .version('10.9.9+golem.9')
  .doc(
    'Run a command provided by a local or downloaded npm package. When tsx or ts-node is not installed in the project, `npx tsx <file>` and `npx ts-node <file>` (and their -e forms) run through the node tool, which runs TypeScript without downloading anything and without type checking; `node <file>` does the same directly.',
  )
  .annotations({ openWorld: false })
  .body((body) =>
    body
      .option('cwd', z.string(), {
        default: DEFAULT_CWD,
        env: 'PWD',
        doc: 'Project directory in which the package command runs. A shell supplies its current directory.',
      })
      .option('registry', z.string(), {
        default: DEFAULT_REGISTRY,
        doc: 'Package registry used to resolve packages.',
      })
      .tail('args', z.string(), {
        separator: '--',
        verbatim: true,
        doc: 'A package command followed by its arguments.',
      })
      .stdout({ required: true, doc: 'Output written by the package command.' })
      .stderr({
        required: true,
        doc: 'Warnings and diagnostics from npx or the package command.',
      })
      .returns(processExitStatusSchema, {
        doc: 'Process exit status; exit_code zero indicates success.',
      }),
  );

async function loadNpm(): Promise<NpmBundle> {
  return evaluateCommonJs(npmBundleSource, NPM_BUNDLED) as NpmBundle;
}

async function runNpm(
  name: 'npm' | 'npx',
  input: { cwd: string; registry: string; args: string[] },
  streams: {
    stdout: WritableStream<Uint8Array>;
    stderr: WritableStream<Uint8Array>;
  },
) {
  const argv = [
    'node',
    name === 'npx'
      ? '/toolchain/npm/node_modules/npm/bin/npx-cli.js'
      : '/toolchain/npm/node_modules/npm/bin/npm-cli.js',
    ...input.args,
  ];
  const invocation = npmInvocation(input.cwd, input.registry);
  const restore = installNpmRecursiveRmPatch(
    NPM_VERSION,
    NPM_VERSION,
    fsPromises,
    { lstatSync, readdirSync, rmdirSync, unlinkSync },
    join,
  );
  const restoreWarnings = installNpmTransportWarningFilter(process);
  const restorePrivateFiles = installPrivateReadOnlyFiles(NPM_ROOT, npmPrivateFiles);
  try {
    return await runCli(
      { argv, cwd: input.cwd, ...invocation, npmExit: true },
      streams,
      async (processFacade) => {
        const loaded = await loadNpm();
        if (name === 'npx') {
          processFacade.argv.splice(
            0,
            processFacade.argv.length,
            ...rewriteNpxArguments(
              processFacade.argv,
              loaded.definitions,
              loaded.shorthands,
              (message) => processFacade.stderr.write(`${message}\n`),
            ),
          );
        }
        await loaded.default(processFacade);
      },
    );
  } finally {
    restorePrivateFiles();
    restoreWarnings();
    restore();
  }
}

function stripEvalTypes(source: string): string {
  // The runtime reports every experimental feature once; the eval transform is an
  // implementation detail of the tool, not a feature the program asked for.
  const emitWarning = process.emitWarning;
  process.emitWarning = ((warning: string | Error, ...rest: unknown[]) => {
    if (rest[0] === 'ExperimentalWarning' && String(warning).startsWith('stripTypeScriptTypes')) {
      return;
    }
    return (emitWarning as (...args: unknown[]) => void).call(process, warning, ...rest);
  }) as typeof process.emitWarning;
  try {
    return stripTypeScriptTypes(source, { mode: 'transform', sourceUrl: '[eval]' });
  } finally {
    process.emitWarning = emitWarning;
  }
}

async function runNode(
  input: { cwd: string; args: string[] },
  streams: {
    stdout: WritableStream<Uint8Array>;
    stderr: WritableStream<Uint8Array>;
  },
) {
  let parsed: NodeInvocation | undefined;
  let parseError: unknown;
  try {
    parsed = parseNodeArguments(input.args);
  } catch (error) {
    parseError = error;
  }
  const cwd = validatedCwd(input.cwd);
  const home = join(cwd, '.golem-home');
  const globalScope = globalThis as typeof globalThis & { require: NodeJS.Require };
  const originalRequire = globalScope.require;
  if (parsed?.kind === 'eval') globalScope.require = createRequire(join(cwd, '[eval]'));
  return runCli(
    {
      argv: parsed?.kind === 'version' ? input.args : (parsed?.argv ?? ['node', ...input.args]),
      cwd,
      environment: {
        HOME: home,
        PATH: `${join(cwd, 'node_modules/.bin')}:/usr/local/bin:/usr/bin:/bin`,
      },
      directories: [home],
      stopOnExit: true,
      waitForRuntimeIdle: true,
    },
    streams,
    async () => {
      if (parseError instanceof NodeOptionError) {
        process.stderr.write(`${parseError.message}\n`);
        process.exit(9);
      }
      if (parseError !== undefined) throw parseError;
      if (parsed!.kind === 'version') {
        process.stdout.write(`${process.version}\n`);
      } else if (parsed!.kind === 'eval') {
        await compileEvalSource(parsed!.source, parsed!.typescript, stripEvalTypes)();
      } else {
        const entry = resolve(cwd, parsed!.entry);
        readFileSync(entry);
        await import(entry);
      }
    },
  ).finally(() => {
    globalScope.require = originalRequire;
  });
}

nodeTool.implement({
  node: async (input, context) => ok({ exit_code: await runNode(input, context) }),
});

npmTool.implement({
  npm: async (input, context) => ok({ exit_code: await runNpm('npm', input, context) }),
});

async function runNpx(
  input: { cwd: string; registry: string; args: string[] },
  streams: {
    stdout: WritableStream<Uint8Array>;
    stderr: WritableStream<Uint8Array>;
  },
) {
  const shim = npxTypeScriptShim(input.args, validatedCwd(input.cwd), existsSync);
  if (shim === undefined) return runNpm('npx', input, streams);
  if (shim.kind === 'error') {
    return runCli({ argv: ['npx', ...input.args], cwd: input.cwd }, streams, () => {
      process.stderr.write(`${shim.message}\n`);
      process.exitCode = 1;
    });
  }
  if (shim.notice !== undefined) {
    const writer = streams.stderr.getWriter();
    try {
      await writer.write(new TextEncoder().encode(`${shim.notice}\n`));
    } finally {
      writer.releaseLock();
    }
  }
  return runNode({ cwd: input.cwd, args: shim.args }, streams);
}

npxTool.implement({
  npx: async (input, context) => ok({ exit_code: await runNpx(input, context) }),
});
