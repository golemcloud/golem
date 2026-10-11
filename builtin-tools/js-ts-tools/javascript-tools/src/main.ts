import { lstatSync, readFileSync, readdirSync, rmdirSync, unlinkSync } from 'node:fs';
import fsPromises from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { ok, s, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod/v4';
import { runCli, validatedCwd } from '../../shared/cli-runtime.js';
import { evaluateCommonJs, installPrivateReadOnlyFiles } from '../../shared/private-vfs.js';
import { npmBundleSource, npmPrivateFiles } from '../../generated/npm-assets.js';
import {
  DEFAULT_REGISTRY,
  installNpmRecursiveRmPatch,
  NPM_VERSION,
  npmInvocation,
} from './npm-runtime.js';
import { rewriteNpxArguments } from './npx-args.js';

const DEFAULT_CWD = '/workspace';
const NPM_ROOT = '/toolchain/npm/node_modules/npm';
const NPM_BUNDLED = '/toolchain/private/npm-cli.cjs';

type NpmBundle = {
  default: (process: NodeJS.Process) => Promise<unknown>;
  definitions: Record<string, { type?: unknown }>;
  shorthands: Record<string, string[]>;
};

const nodeTool = toolDefinition('node', { requiresFilesystem: true })
  .version('0.1.4')
  .doc("Run a JavaScript file or inline expression with Golem's Node-compatible runtime.")
  .annotations({ openWorld: false })
  .body((body) =>
    body
      .option('cwd', z.string(), {
        default: DEFAULT_CWD,
        doc: 'Directory used to resolve files and relative paths.',
      })
      .tail('args', z.string(), {
        separator: '--',
        verbatim: true,
        doc: 'A script path and its arguments, an inline expression with -e, or a Node option.',
      })
      .stdout({ required: true, doc: 'Output written by the JavaScript program.' })
      .stderr({ required: true, doc: 'Diagnostics written by the JavaScript program.' })
      .returns(s.s32(), { doc: 'Process exit code; zero indicates success.' }),
  );

const npmTool = toolDefinition('npm', { requiresFilesystem: true })
  .version('10.9.9+golem.4')
  .doc('Install and manage JavaScript packages and run package scripts with npm.')
  .annotations({ openWorld: false })
  .body((body) =>
    body
      .option('cwd', z.string(), {
        default: DEFAULT_CWD,
        doc: 'Project directory in which npm runs.',
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
      .returns(s.s32(), { doc: 'Process exit code; zero indicates success.' }),
  );

const npxTool = toolDefinition('npx', { requiresFilesystem: true })
  .version('10.9.9+golem.4')
  .doc('Run a command provided by a local or downloaded npm package.')
  .annotations({ openWorld: false })
  .body((body) =>
    body
      .option('cwd', z.string(), {
        default: DEFAULT_CWD,
        doc: 'Project directory in which the package command runs.',
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
      .stderr({ required: true, doc: 'Warnings and diagnostics from npx or the package command.' })
      .returns(s.s32(), { doc: 'Process exit code; zero indicates success.' }),
  );

async function loadNpm(): Promise<NpmBundle> {
  return evaluateCommonJs(npmBundleSource, NPM_BUNDLED) as NpmBundle;
}

async function runNpm(
  name: 'npm' | 'npx',
  input: { cwd: string; registry: string; args: string[] },
  streams: { stdout: WritableStream<Uint8Array>; stderr: WritableStream<Uint8Array> },
) {
  const argv = [
    'node',
    name === 'npx'
      ? '/toolchain/npm/node_modules/npm/bin/npx-cli.js'
      : '/toolchain/npm/node_modules/npm/bin/npm-cli.js',
    ...input.args,
  ];
  const restore = installNpmRecursiveRmPatch(
    NPM_VERSION,
    NPM_VERSION,
    fsPromises,
    { lstatSync, readdirSync, rmdirSync, unlinkSync },
    join,
  );
  const restorePrivateFiles = installPrivateReadOnlyFiles(NPM_ROOT, npmPrivateFiles);
  const invocation = npmInvocation(input.cwd, input.registry);
  try {
    return await runCli(
      { argv, cwd: input.cwd, ...invocation },
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
    restore();
  }
}

function parseNodeArguments(args: string[]): { source?: string; entry?: string; argv: string[] } {
  if (args[0] === '--version' || args[0] === '-v') return { source: '', argv: args };
  if (args[0] === '--eval' || args[0] === '-e') {
    if (args[1] === undefined) throw new Error('node --eval requires source');
    return { source: args[1], argv: ['node', ...args.slice(2)] };
  }
  if (!args[0]) throw new Error('node requires a file or --eval source');
  return { entry: args[0], argv: ['node', args[0], ...args.slice(1)] };
}

async function runNode(
  input: { cwd: string; args: string[] },
  streams: { stdout: WritableStream<Uint8Array>; stderr: WritableStream<Uint8Array> },
) {
  let parsed: ReturnType<typeof parseNodeArguments> | undefined;
  let parseError: unknown;
  try {
    parsed = parseNodeArguments(input.args);
  } catch (error) {
    parseError = error;
  }
  const cwd = validatedCwd(input.cwd);
  const home = `${cwd}/.golem-home`;
  return runCli(
    {
      argv: parsed?.argv ?? ['node', ...input.args],
      cwd,
      environment: {
        HOME: home,
        PATH: `${cwd}/node_modules/.bin:/usr/local/bin:/usr/bin:/bin`,
      },
      directories: [home],
      stopOnExit: true,
      waitForRuntimeIdle: true,
    },
    streams,
    async () => {
      if (parseError !== undefined) throw parseError;
      if (input.args[0] === '--version' || input.args[0] === '-v') {
        process.stdout.write(`${process.version}\n`);
        return;
      }
      const entry = parsed!.entry ? resolve(cwd, parsed!.entry) : undefined;
      if (parsed!.source !== undefined) {
        const AsyncFunction = Object.getPrototypeOf(async () => undefined).constructor as new (
          ...args: string[]
        ) => () => Promise<unknown>;
        await new AsyncFunction(parsed!.source)();
      } else {
        readFileSync(entry!);
        await import(entry!);
      }
    },
  );
}

nodeTool.implement({
  node: async (input, context) => ok(await runNode(input, context)),
});

npmTool.implement({
  npm: async (input, context) => ok(await runNpm('npm', input, context)),
});

npxTool.implement({
  npx: async (input, context) => ok(await runNpm('npx', input, context)),
});
