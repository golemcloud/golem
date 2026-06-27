import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(fileURLToPath(new URL('..', import.meta.url)));
const exportsPath = path.join(packageRoot, 'types', 'exports.d.ts');

const lines = fs.readFileSync(exportsPath, 'utf8').split('\n');
const seenGuestTypeAliases = new Set();
let inGuestNamespace = false;

const patched = lines
  .map((line) =>
    line.startsWith('    export function invoke(toolName:')
      ? line.replace('export function invoke(', 'export function invokeTool(')
      : line,
  )
  .filter((line) => {
    if (line === '  export namespace guest {') {
      inGuestNamespace = true;
      return true;
    }

    if (inGuestNamespace && line === '  }') {
      inGuestNamespace = false;
      return true;
    }

    const alias = inGuestNamespace
      ? line.match(/^    export type (\w+)(?:<[^>]+>)? = /)?.[1]
      : undefined;
    if (alias) {
      if (seenGuestTypeAliases.has(alias)) return false;
      seenGuestTypeAliases.add(alias);
    }

    return true;
  });

fs.writeFileSync(exportsPath, patched.join('\n'), 'utf8');
