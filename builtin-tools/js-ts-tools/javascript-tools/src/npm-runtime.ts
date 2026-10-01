import type { Dirent, Stats } from 'node:fs';
import { validatedCwd } from '../../shared/cli-runtime.js';

export const NPM_VERSION = '10.9.9';
export const DEFAULT_REGISTRY = 'https://registry.npmjs.org/';

type FsPromisesWithRm = {
  rm: (path: string, options?: { force?: boolean; recursive?: boolean }) => Promise<void>;
};

type SyncRemovalFs = {
  lstatSync(path: string): Stats;
  readdirSync(path: string, options: { withFileTypes: true }): Dirent<string>[];
  rmdirSync(path: string): void;
  unlinkSync(path: string): void;
};

export function normalizedRegistry(value: string): string {
  const url = new URL(value);
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error('registry must use HTTP or HTTPS');
  }
  if (url.username || url.password) throw new Error('registry must not contain credentials');
  url.hash = '';
  url.search = '';
  if (!url.pathname.endsWith('/')) url.pathname += '/';
  return url.toString();
}

export function npmInvocation(cwdValue: string, registryValue: string) {
  const cwd = validatedCwd(cwdValue);
  const home = `${cwd}/.golem-home`;
  const cache = `${cwd}/.golem-npm-cache`;
  const prefix = `${cwd}/.golem-npm-prefix`;
  return {
    environment: {
      HOME: home,
      NODE: process.execPath,
      NPM: '/toolchain/npm/node_modules/npm/bin/npm-cli.js',
      NPM_CONFIG_AUDIT: 'false',
      NPM_CONFIG_CACHE: cache,
      NPM_CONFIG_FUND: 'false',
      NPM_CONFIG_PREFIX: prefix,
      NPM_CONFIG_REGISTRY: normalizedRegistry(registryValue),
      NPM_CONFIG_UPDATE_NOTIFIER: 'false',
      PATH: `${cwd}/node_modules/.bin:/usr/local/bin:/usr/bin:/bin`,
    },
    directories: [home, cache, prefix],
  };
}

export function installNpmRecursiveRmPatch(
  packageVersion: string,
  expectedVersion: string,
  promises: FsPromisesWithRm,
  syncFs: SyncRemovalFs,
  joinPath: (left: string, right: string) => string,
) {
  const id = 'npm-recursive-rm-symlink-eacces';
  if (packageVersion !== expectedVersion) {
    throw new Error(`${id} only supports npm ${expectedVersion}; received npm ${packageVersion}`);
  }
  const originalRm = promises.rm;
  const boundRm = originalRm.bind(promises);
  let restored = false;

  const removeTree = (path: string, force: boolean): void => {
    let stat: Stats;
    try {
      stat = syncFs.lstatSync(path);
    } catch (error) {
      if (force && (error as NodeJS.ErrnoException)?.code === 'ENOENT') return;
      throw error;
    }
    if (stat.isSymbolicLink() || !stat.isDirectory()) {
      syncFs.unlinkSync(path);
      return;
    }

    let entries: Dirent<string>[];
    try {
      entries = syncFs.readdirSync(path, { withFileTypes: true });
    } catch (error) {
      const code = (error as NodeJS.ErrnoException)?.code;
      if (force && code === 'ENOENT') return;
      if (code === 'ENOTDIR') {
        syncFs.unlinkSync(path);
        return;
      }
      throw error;
    }
    for (const entry of entries) {
      const child = joinPath(path, entry.name);
      if (entry.isDirectory()) removeTree(child, force);
      else syncFs.unlinkSync(child);
    }
    syncFs.rmdirSync(path);
  };

  promises.rm = async (path, options) => {
    try {
      await boundRm(path, options);
    } catch (error) {
      if (!options?.recursive || (error as NodeJS.ErrnoException)?.code !== 'EACCES') throw error;
      removeTree(path, options.force ?? false);
    }
  };

  return () => {
    if (restored) return;
    promises.rm = originalRm;
    restored = true;
  };
}
