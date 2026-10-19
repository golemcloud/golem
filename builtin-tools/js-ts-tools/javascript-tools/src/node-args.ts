export type NodeInvocation =
  | { kind: 'version' }
  | { kind: 'eval'; source: string; typescript: boolean | 'detect'; argv: string[] }
  | { kind: 'entry'; entry: string; argv: string[] };

export class NodeOptionError extends Error {
  readonly name = 'NodeOptionError';
}

// Accepted so that common Node invocations work, and ignored: the runtime always transforms
// TypeScript and keeps source maps, and the tool does not filter warnings.
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

// Node reads its own options before the script and, with --eval, also between the source and
// the first program argument or `--`.
export function parseNodeArguments(args: string[]): NodeInvocation {
  let inputType: string | undefined;
  let source: string | undefined;
  let i = 0;
  const value = (option: string) => {
    const next = args[++i];
    if (next === undefined) throw new NodeOptionError(`node: ${option} requires an argument`);
    return next;
  };
  for (; i < args.length; i++) {
    const arg = args[i]!;
    if (arg === '--version' || arg === '-v') return { kind: 'version' };
    if (arg === '--eval' || arg === '-e') source = value(arg);
    else if (arg.startsWith('--eval=')) source = arg.slice('--eval='.length);
    else if (arg === '--input-type') inputType = value(arg);
    else if (arg.startsWith('--input-type=')) inputType = arg.slice('--input-type='.length);
    else if (IGNORED_OPTIONS.has(arg)) continue;
    else if (arg === '--') {
      i++;
      break;
    } else if (arg.startsWith('-')) throw new NodeOptionError(`node: bad option: ${arg}`);
    else break;
  }
  const rest = args.slice(i);
  if (source !== undefined) {
    const typescript = inputType === undefined ? 'detect' : inputTypeIsTypeScript(inputType);
    return { kind: 'eval', source, typescript, argv: ['node', ...rest] };
  }
  const entry = rest[0];
  if (entry === undefined) throw new NodeOptionError('node: expected a script file or --eval source');
  return { kind: 'entry', entry, argv: ['node', ...rest] };
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
