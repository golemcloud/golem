import { dirname, join } from 'node:path';
import { DEFAULT_REGISTRY, npmInvocation } from './npm-runtime.js';

export type NpxTypeScriptShim =
  | { kind: 'node'; args: string[]; notice?: string }
  | { kind: 'error'; message: string };

type ShimmedCommand = 'tsx' | 'ts-node';

// Only the npx options that do not change which package runs. Any other option, such as
// --package or --call, leaves the invocation to npm.
const PASSIVE_NPX_OPTIONS = new Set([
  '-y',
  '--yes',
  '--yes=true',
  '--no',
  '--yes=false',
  '--no-install',
  '-q',
  '--quiet',
]);

const IGNORED_COMMAND_OPTIONS: Record<ShimmedCommand, Set<string>> = {
  tsx: new Set(),
  'ts-node': new Set(['-T', '--transpile-only']),
};

const TS_NODE_NOTICE =
  'npx ts-node (golem built-in): type checking is skipped; use the tsc tool to type-check.';

function installedBin(cwd: string, command: string, exists: (path: string) => boolean): boolean {
  if (exists(join(npmInvocation(cwd, DEFAULT_REGISTRY).environment.NPM_CONFIG_PREFIX, 'bin', command))) {
    return true;
  }
  for (let directory = cwd; ; directory = dirname(directory)) {
    if (exists(join(directory, 'node_modules', '.bin', command))) return true;
    if (directory === dirname(directory)) return false;
  }
}

function unsupported(command: ShimmedCommand, what: string): NpxTypeScriptShim {
  return {
    kind: 'error',
    message:
      `npx ${command} (golem built-in): ${what} is not supported. ` +
      `${command} is not installed, so npx runs the file with the node tool, which runs TypeScript without flags. ` +
      `Run \`node <file>\` directly, or install the real package with \`npm install -D ${command}\`.`,
  };
}

// The runtime transforms TypeScript itself, and `tsx` depends on a native esbuild binary that it
// cannot execute. When the requested command is not installed, npx runs the common `tsx` and
// `ts-node` forms through the node tool instead of downloading the package.
export function npxTypeScriptShim(
  args: string[],
  cwd: string,
  exists: (path: string) => boolean,
): NpxTypeScriptShim | undefined {
  let index = 0;
  while (index < args.length && PASSIVE_NPX_OPTIONS.has(args[index]!)) index++;
  if (args[index] === '--') index++;
  const command = args[index];
  if (command !== 'tsx' && command !== 'ts-node') return undefined;
  if (installedBin(cwd, command, exists)) return undefined;

  const notice = command === 'ts-node' ? TS_NODE_NOTICE : undefined;
  const ignored = IGNORED_COMMAND_OPTIONS[command];
  for (let i = index + 1; i < args.length; i++) {
    const arg = args[i]!;
    if (ignored.has(arg)) continue;
    if (arg === '-e' || arg === '--eval') {
      const source = args[i + 1];
      if (source === undefined) return unsupported(command, `${arg} without source`);
      // Like ts-node, read options after the source up to `--` or the first program argument.
      let rest = i + 2;
      for (; rest < args.length; rest++) {
        const option = args[rest]!;
        if (option === '--') {
          rest++;
          break;
        }
        if (!option.startsWith('-')) break;
        if (!ignored.has(option)) return unsupported(command, `the option ${option}`);
      }
      return {
        kind: 'node',
        args: ['--input-type=commonjs-typescript', '--eval', source, '--', ...args.slice(rest)],
        notice,
      };
    }
    if (arg === '--') {
      if (args[i + 1] === undefined) break;
      return { kind: 'node', args: ['--', ...args.slice(i + 1)], notice };
    }
    if (arg.startsWith('-')) return unsupported(command, `the option ${arg}`);
    if (command === 'tsx' && arg === 'watch') return unsupported(command, 'watch mode');
    return { kind: 'node', args: ['--', ...args.slice(i)], notice };
  }
  return unsupported(command, 'the interactive REPL');
}
