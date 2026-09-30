import { readFileSync, readdirSync, rmdirSync, unlinkSync } from 'node:fs';
import fsPromises from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { ok, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod/v4';
import {
  DEFAULT_CWD,
  DEFAULT_MAX_OUTPUT_BYTES,
  DEFAULT_REGISTRY,
  NPM_VERSION,
  ToolExecutionResultSchema,
} from '../../shared/contracts.js';
import { runInDirectRuntime } from '../../shared/direct-runtime.js';
import { installNpmRecursiveRmPatch } from '../../shared/npm-compat.js';
import { rewriteNpxArguments } from '../../shared/npx-args.js';
import { evaluateCommonJs, installPrivateReadOnlyFiles } from '../../shared/private-vfs.js';
import { npmBundleSource, npmPrivateFiles } from '../../generated/npm-assets.js';

const NPM_ROOT = '/toolchain/npm/node_modules/npm';
const NPM_BUNDLED = '/toolchain/private/npm-cli.cjs';

type NpmBundle = {
  default: (process: NodeJS.Process) => Promise<unknown>;
  definitions: Record<string, { type?: unknown }>;
  shorthands: Record<string, string[]>;
};

const nodeTool = toolDefinition('node', { requiresFilesystem: true })
  .version('0.1.0')
  .doc(
    "Golem's QuickJS-based Node-compatible JavaScript runner. This is not a Node.js distribution.",
  )
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
        doc: 'Arguments passed unchanged to the command.',
      })
      .returns(ToolExecutionResultSchema),
  );

const npmTool = toolDefinition('npm', { requiresFilesystem: true })
  .version(NPM_VERSION)
  .doc('Run the pinned upstream npm CLI in an isolated Golem tool sidecar.')
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
        doc: 'Arguments passed unchanged to the command.',
      })
      .returns(ToolExecutionResultSchema),
  );

const npxTool = toolDefinition('npx', { requiresFilesystem: true })
  .version(NPM_VERSION)
  .doc('Run npm exec with the argument rewriting of the pinned upstream npx CLI.')
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
        doc: 'Arguments passed unchanged to the command.',
      })
      .returns(ToolExecutionResultSchema),
  );

async function loadNpm(): Promise<NpmBundle> {
  return evaluateCommonJs(npmBundleSource, NPM_BUNDLED) as NpmBundle;
}

async function runNpm(
  name: 'npm' | 'npx',
  input: {
    cwd: string;
    registry: string;
    maxOutputBytes: number;
    args: string[];
  },
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
    { readdirSync, rmdirSync, unlinkSync },
    join,
  );
  const restorePrivateFiles = installPrivateReadOnlyFiles(NPM_ROOT, npmPrivateFiles);
  try {
    return await runInDirectRuntime(
      {
        argv,
        cwd: input.cwd,
        registry: input.registry,
        maxOutputBytes: input.maxOutputBytes,
        version: NPM_VERSION,
      },
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

async function runNode(input: {
  cwd: string;
  registry: string;
  maxOutputBytes: number;
  args: string[];
}) {
  const parsed = parseNodeArguments(input.args);
  const version = process.version;
  if (input.args[0] === '--version' || input.args[0] === '-v') {
    return {
      exitCode: 0,
      version,
      overflowed: false,
      stdout: `${version}\n`,
      stderr: '',
    };
  }
  const entry = parsed.entry ? resolve(input.cwd, parsed.entry) : undefined;
  return runInDirectRuntime(
    {
      argv: parsed.argv,
      cwd: input.cwd,
      registry: input.registry,
      maxOutputBytes: input.maxOutputBytes,
      version,
      stopOnExit: true,
    },
    async () => {
      if (parsed.source !== undefined) {
        const AsyncFunction = Object.getPrototypeOf(async () => undefined).constructor as new (
          ...args: string[]
        ) => () => Promise<unknown>;
        await new AsyncFunction(parsed.source)();
      } else {
        readFileSync(entry!);
        await import(entry!);
      }
    },
  );
}

nodeTool.implement({
  node: async (input) => ok(await runNode(input)),
});

npmTool.implement({
  npm: async (input) => ok(await runNpm('npm', input)),
});

npxTool.implement({
  npx: async (input) => ok(await runNpm('npx', input)),
});
