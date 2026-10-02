import fs from 'node:fs';
import fsPromises from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { Readable } from 'node:stream';

type PrivateFiles = Readonly<Record<string, string>>;

function fsError(code: 'ENOENT' | 'ENOSYS', operation: string, file: string): NodeJS.ErrnoException {
  const error = new Error(`${code}: ${operation} '${file}'`) as NodeJS.ErrnoException;
  error.code = code;
  error.errno = code === 'ENOENT' ? -2 : -38;
  error.path = file;
  error.syscall = operation;
  return error;
}

function requestedEncoding(options: unknown): BufferEncoding | undefined {
  if (typeof options === 'string') return options as BufferEncoding;
  if (options && typeof options === 'object') {
    const value = (options as { encoding?: unknown }).encoding;
    if (typeof value === 'string') return value as BufferEncoding;
  }
  return undefined;
}

/**
 * Exposes component-owned files to an upstream CLI without materializing them in the owner
 * filesystem. Paths below `rootValue` are fail-closed: absent files never fall through to the
 * caller's filesystem.
 */
export function installPrivateReadOnlyFiles(rootValue: string, files: PrivateFiles): () => void {
  const root = path.posix.resolve(rootValue);
  const directories = new Set<string>(['']);
  for (const name of Object.keys(files)) {
    for (let directory = path.posix.dirname(name); directory !== '.'; directory = path.posix.dirname(directory)) {
      directories.add(directory);
    }
  }

  const relative = (value: unknown): string | undefined => {
    if (typeof value !== 'string') return undefined;
    const normalized = path.posix.resolve(value);
    if (normalized === root) return '';
    return normalized.startsWith(`${root}/`) ? normalized.slice(root.length + 1) : undefined;
  };
  const kind = (value: unknown): { name: string; directory: boolean } | undefined => {
    const name = relative(value);
    if (name === undefined) return undefined;
    if (Object.hasOwn(files, name)) return { name, directory: false };
    if (directories.has(name)) return { name, directory: true };
    throw fsError('ENOENT', 'open', String(value));
  };
  const privateStat = (value: unknown) => {
    const item = kind(value);
    if (!item) return undefined;
    const size = item.directory ? 0 : Buffer.byteLength(files[item.name]!);
    return {
      size,
      mode: item.directory ? 0o40555 : 0o100444,
      mtimeMs: 0,
      ctimeMs: 0,
      birthtimeMs: 0,
      isFile: () => !item.directory,
      isDirectory: () => item.directory,
      isSymbolicLink: () => false,
      isBlockDevice: () => false,
      isCharacterDevice: () => false,
      isFIFO: () => false,
      isSocket: () => false,
    };
  };
  const privateRead = (value: unknown, options?: unknown) => {
    const item = kind(value);
    if (!item || item.directory) throw fsError('ENOENT', 'read', String(value));
    const contents = files[item.name]!;
    return requestedEncoding(options) ? contents : Buffer.from(contents);
  };
  const privateList = (value: unknown, options?: unknown) => {
    const item = kind(value);
    if (!item || !item.directory) throw fsError('ENOENT', 'readdir', String(value));
    const prefix = item.name ? `${item.name}/` : '';
    const names = new Set<string>();
    for (const name of [...Object.keys(files), ...directories]) {
      if (!name.startsWith(prefix)) continue;
      const child = name.slice(prefix.length).split('/')[0];
      if (child) names.add(child);
    }
    const result = [...names].sort();
    const withFileTypes =
      options && typeof options === 'object' && (options as { withFileTypes?: boolean }).withFileTypes;
    if (!withFileTypes) return result;
    return result.map((name) => {
      const child = item.name ? `${item.name}/${name}` : name;
      const directory = directories.has(child);
      return {
        name,
        isFile: () => !directory,
        isDirectory: () => directory,
        isSymbolicLink: () => false,
      };
    });
  };
  const privateRealpath = (value: unknown) => {
    kind(value);
    return path.posix.resolve(String(value));
  };

  const mutableFs = fs as unknown as Record<string, any>;
  const mutablePromises = fsPromises as unknown as Record<string, any>;
  const names = [
    'access',
    'accessSync',
    'createReadStream',
    'existsSync',
    'lstat',
    'lstatSync',
    'open',
    'openSync',
    'readFile',
    'readFileSync',
    'readdir',
    'readdirSync',
    'realpath',
    'realpathSync',
    'stat',
    'statSync',
  ];
  const promiseNames = ['access', 'lstat', 'open', 'readFile', 'readdir', 'realpath', 'stat'];
  const originalFs = Object.fromEntries(names.map((name) => [name, mutableFs[name]]));
  const originalPromises = Object.fromEntries(
    promiseNames.map((name) => [name, mutablePromises[name]]),
  );
  const fallback = (name: string, args: unknown[], load: () => unknown) =>
    relative(args[0]) === undefined ? originalFs[name].apply(fs, args) : load();

  mutableFs.existsSync = (value: unknown) => {
    if (relative(value) === undefined) return originalFs.existsSync.call(fs, value);
    try {
      kind(value);
      return true;
    } catch {
      return false;
    }
  };
  mutableFs.statSync = (...args: unknown[]) => fallback('statSync', args, () => privateStat(args[0]));
  mutableFs.lstatSync = (...args: unknown[]) => fallback('lstatSync', args, () => privateStat(args[0]));
  mutableFs.readFileSync = (...args: unknown[]) =>
    fallback('readFileSync', args, () => privateRead(args[0], args[1]));
  mutableFs.readdirSync = (...args: unknown[]) =>
    fallback('readdirSync', args, () => privateList(args[0], args[1]));
  const realpathSync = (...args: unknown[]) =>
    fallback('realpathSync', args, () => privateRealpath(args[0]));
  realpathSync.native = (...args: unknown[]) =>
    relative(args[0]) === undefined
      ? (originalFs.realpathSync.native ?? originalFs.realpathSync).apply(fs, args)
      : privateRealpath(args[0]);
  mutableFs.realpathSync = realpathSync;
  mutableFs.accessSync = (...args: unknown[]) => fallback('accessSync', args, () => kind(args[0]));
  mutableFs.openSync = (...args: unknown[]) =>
    fallback('openSync', args, () => {
      throw fsError('ENOSYS', 'open', String(args[0]));
    });
  mutableFs.createReadStream = (...args: unknown[]) =>
    fallback('createReadStream', args, () => Readable.from([privateRead(args[0])]));

  const callbackOperation = (
    name: string,
    load: (args: unknown[]) => unknown,
    original = originalFs[name],
  ) =>
    (...args: unknown[]) => {
      if (relative(args[0]) === undefined) return original.apply(fs, args);
      const candidate = args[args.length - 1];
      const done = (typeof candidate === 'function' ? candidate : undefined) as
        | ((error: unknown, value?: unknown) => void)
        | undefined;
      if (!done) throw new TypeError(`${name} requires a callback`);
      try {
        const value = load(args);
        queueMicrotask(() => done(null, value));
      } catch (error) {
        queueMicrotask(() => done(error));
      }
    };
  mutableFs.stat = callbackOperation('stat', (args) => privateStat(args[0]));
  mutableFs.lstat = callbackOperation('lstat', (args) => privateStat(args[0]));
  mutableFs.readFile = callbackOperation('readFile', (args) => privateRead(args[0], args[1]));
  mutableFs.readdir = callbackOperation('readdir', (args) => privateList(args[0], args[1]));
  const realpath = callbackOperation('realpath', (args) => privateRealpath(args[0])) as ReturnType<
    typeof callbackOperation
  > & { native: ReturnType<typeof callbackOperation> };
  realpath.native = callbackOperation(
    'realpath.native',
    (args) => privateRealpath(args[0]),
    originalFs.realpath.native ?? originalFs.realpath,
  );
  mutableFs.realpath = realpath;
  mutableFs.access = callbackOperation('access', (args) => kind(args[0]));
  mutableFs.open = callbackOperation('open', (args) => {
    throw fsError('ENOSYS', 'open', String(args[0]));
  });

  const promiseOperation = (name: string, load: (args: unknown[]) => unknown) =>
    async (...args: unknown[]) =>
      relative(args[0]) === undefined
        ? originalPromises[name].apply(fsPromises, args)
        : load(args);
  mutablePromises.stat = promiseOperation('stat', (args) => privateStat(args[0]));
  mutablePromises.lstat = promiseOperation('lstat', (args) => privateStat(args[0]));
  mutablePromises.readFile = promiseOperation('readFile', (args) => privateRead(args[0], args[1]));
  mutablePromises.readdir = promiseOperation('readdir', (args) => privateList(args[0], args[1]));
  mutablePromises.realpath = promiseOperation('realpath', (args) => privateRealpath(args[0]));
  mutablePromises.access = promiseOperation('access', (args) => kind(args[0]));
  mutablePromises.open = promiseOperation('open', (args) => {
    throw fsError('ENOSYS', 'open', String(args[0]));
  });

  return () => {
    Object.assign(mutableFs, originalFs);
    Object.assign(mutablePromises, originalPromises);
  };
}

export function evaluateCommonJs(source: string, filename: string): unknown {
  const module = { exports: {} as unknown };
  const execute = new Function(
    'require',
    'module',
    'exports',
    '__filename',
    '__dirname',
    `${source}\n//# sourceURL=${filename}`,
  );
  execute(
    createRequire(filename),
    module,
    module.exports,
    filename,
    path.posix.dirname(filename),
  );
  return module.exports;
}
