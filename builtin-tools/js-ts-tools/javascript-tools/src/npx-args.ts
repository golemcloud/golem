type NpxDefinitions = Record<string, { type?: unknown }>;

export function rewriteNpxArguments(
  argv: string[],
  definitions: NpxDefinitions,
  shorthands: Record<string, string[]>,
  reportRemoved: (message: string) => void,
): string[] {
  const result = [...argv];
  result[1] = '/toolchain/npm/node_modules/npm/bin/npm-cli.js';
  result.splice(2, 0, 'exec');
  const removedSwitches = new Set(['always-spawn', 'ignore-existing', 'shell-auto-fallback']);
  const removedOpts = new Set(['npm', 'node-arg', 'n']);
  const removed = new Set([...removedSwitches, ...removedOpts]);
  const npmSwitches = Object.entries(definitions)
    .filter(([, { type }]) => type === Boolean || (Array.isArray(type) && type.includes(Boolean)))
    .map(([key]) => key);
  const switches = new Set([
    ...removedSwitches,
    ...npmSwitches,
    'no-install',
    'quiet',
    'q',
    'version',
    'v',
    'help',
    'h',
  ]);
  const opts = new Set([
    ...removedOpts,
    'package',
    'p',
    'cache',
    'userconfig',
    'call',
    'c',
    'shell',
  ]);
  let sawRemovedFlags = false;
  for (let i = 3; i < result.length; i++) {
    const arg = result[i]!;
    if (arg === '--') break;
    if (!/^-/.test(arg)) {
      result.splice(i, 0, '--');
      break;
    }
    const [key = '', ...valueParts] = arg.replace(/^-+/, '').split('=');
    if (key === 'p') result[i] = ['--package', ...valueParts].join('=');
    else if (key === 'shell') result[i] = ['--script-shell', ...valueParts].join('=');
    else if (key === 'no-install') result[i] = '--yes=false';
    else if (shorthands[key] && !removed.has(key)) {
      const expanded = [...shorthands[key]];
      if (valueParts.length) expanded.push(valueParts.join('='));
      result.splice(i, 1, ...expanded);
      i--;
      continue;
    }
    if (removed.has(key)) {
      reportRemoved(`npx: the --${key} argument has been removed.`);
      sawRemovedFlags = true;
      result.splice(i, 1);
      i--;
    }
    if (
      valueParts.length === 0 &&
      !switches.has(key) &&
      (opts.has(key) || !/^-/.test(result[i + 1] ?? ''))
    ) {
      if (removed.has(key)) result.splice(i + 1, 1);
      else i++;
    }
  }
  if (sawRemovedFlags) reportRemoved('See `npm help exec` for more information');
  return result;
}
