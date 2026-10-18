export type NodeInvocation =
  | { kind: 'version' }
  | { kind: 'eval'; source: string; typescript: boolean | 'detect'; argv: string[] }
  | { kind: 'entry'; entry: string; argv: string[] };

export class NodeOptionError extends Error {
  readonly name = 'NodeOptionError';
}

// The runtime always removes TypeScript types and keeps source maps, so these Node options
// only confirm the behaviour.
const IGNORED_OPTIONS = new Set([
  '--experimental-strip-types',
  '--experimental-transform-types',
  '--no-experimental-strip-types',
  '--no-warnings',
  '--enable-source-maps',
]);

function inputTypeIsTypeScript(value: string): boolean {
  if (value === 'commonjs') return false;
  if (value === 'commonjs-typescript') return true;
  if (value === 'module' || value === 'module-typescript') {
    const extension = value === 'module' ? '.mjs' : '.mts';
    throw new NodeOptionError(
      `node: --input-type=${value} is not supported for --eval; write the code to a ${extension} file and run that file`,
    );
  }
  throw new NodeOptionError(
    `node: --input-type must be "commonjs", "commonjs-typescript", "module" or "module-typescript", received "${value}"`,
  );
}

export function parseNodeArguments(args: string[]): NodeInvocation {
  let inputType: string | undefined;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (arg === '--version' || arg === '-v') return { kind: 'version' };
    if (arg === '--eval' || arg === '-e') {
      const source = args[i + 1];
      if (source === undefined) throw new NodeOptionError(`node: ${arg} requires an argument`);
      const typescript = inputType === undefined ? 'detect' : inputTypeIsTypeScript(inputType);
      return { kind: 'eval', source, typescript, argv: ['node', ...args.slice(i + 2)] };
    }
    if (arg === '--input-type') {
      const value = args[i + 1];
      if (value === undefined) throw new NodeOptionError('node: --input-type requires an argument');
      inputType = value;
      i++;
      continue;
    }
    if (arg.startsWith('--input-type=')) {
      inputType = arg.slice('--input-type='.length);
      continue;
    }
    if (IGNORED_OPTIONS.has(arg)) continue;
    if (arg === '--') {
      const entry = args[i + 1];
      if (entry === undefined) break;
      return { kind: 'entry', entry, argv: ['node', ...args.slice(i + 1)] };
    }
    if (arg.startsWith('-')) throw new NodeOptionError(`node: bad option: ${arg}`);
    return { kind: 'entry', entry: arg, argv: ['node', ...args.slice(i)] };
  }
  throw new NodeOptionError('node: expected a script file or --eval source');
}

type AsyncFunctionConstructor = new (...args: string[]) => () => Promise<unknown>;

const AsyncFunction = Object.getPrototypeOf(async () => undefined)
  .constructor as AsyncFunctionConstructor;

// Node evaluates `--eval` source as JavaScript first and, when it does not parse, as TypeScript.
// Only the construction of the function parses the source, so a SyntaxError thrown while the
// program runs never causes a TypeScript retry.
export function compileEvalSource(
  source: string,
  typescript: boolean | 'detect',
  stripTypes: (source: string) => string,
): () => Promise<unknown> {
  if (typescript === true) return new AsyncFunction(stripTypes(source));
  try {
    return new AsyncFunction(source);
  } catch (error) {
    if (typescript === false || !(error instanceof SyntaxError)) throw error;
    let stripped: string;
    try {
      stripped = stripTypes(source);
    } catch {
      throw error;
    }
    return new AsyncFunction(stripped);
  }
}
