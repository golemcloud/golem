import type { Dirent } from 'node:fs';

type FsPromisesWithRm = {
  rm: (path: string, options?: { force?: boolean; recursive?: boolean }) => Promise<void>;
};

type SyncRemovalFs = {
  readdirSync(path: string, options: { withFileTypes: true }): Dirent<string>[];
  rmdirSync(path: string): void;
  unlinkSync(path: string): void;
};

export function installNpmRecursiveRmPatch(
  packageVersion: string,
  expectedVersion: string,
  promises: FsPromisesWithRm,
  syncFs: SyncRemovalFs,
  joinPath: (left: string, right: string) => string,
) {
  const id = 'npm-recursive-rm-symlink-eacces';
  if (packageVersion !== expectedVersion) {
    throw new Error(
      `${id} only supports npm ${expectedVersion}; received npm ${packageVersion}`,
    );
  }
  const originalRm = promises.rm;
  const boundRm = originalRm.bind(promises);
  let restored = false;

  const removeTree = (path: string, force: boolean): void => {
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
