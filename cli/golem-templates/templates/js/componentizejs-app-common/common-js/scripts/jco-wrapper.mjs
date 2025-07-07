// Thin wrapper around jco, exposing additional options for type generation

import { program, Option } from 'commander';

import { guestTypes } from '../../node_modules/@bytecodealliance/jco/src/cmd/transpile.js';

program
  .name('typegen')
  .usage('<command> [options]')

program.command('guest-types')
  .description('(experimental) Generate guest types for the given WIT')
  .usage('<wit-path> -o <out-dir>')
  .argument('<wit-path>', 'path to a WIT file or directory')
  .option('--name <name>', 'custom output name')
  .option('-n, --world-name <world>', 'WIT world to generate types for')
  .requiredOption('-o, --out-dir <out-dir>', 'output directory')
  .option('-q, --quiet', 'disable output summary')
  .option('--feature <feature>', 'enable one specific WIT feature (repeatable)', collectOptions, [])
  .option('--all-features', 'enable all features')
  .addOption(new Option('--async-mode [mode]', 'EXPERIMENTAL: use async imports and exports').choices(['sync', 'jspi']).preset('sync'))
  .option('--async-wasi-exports', 'EXPERIMENTAL: async component exports from WASI interfaces')
  .option('--async-exports <exports...>', 'EXPERIMENTAL: async component exports (examples: "wasi:cli/run@#run", "handle")')
  .option('--async-imports <imports...>', 'EXPERIMENTAL: async component imports (examples: "wasi:io/poll@0.2.0#poll", "wasi:io/poll#[method]pollable.block")')
  .action(asyncAction(guestTypes));

program.showHelpAfterError();

program.parse();

function collectOptions(value, previous) {
  return previous.concat([value]);
}

function asyncAction (cmd) {
  return function () {
    const args = [...arguments];
    (async () => {
      try {
        await cmd.apply(null, args);
      }
      catch (e) {
        process.stdout.write(`(jco ${cmd.name}) `);
        if (typeof e === 'string') {
          console.error(`{red.bold Error}: ${e}\n`);
        } else {
          console.error(e);
        }
        process.exit(1);
      }
    })();
  };
}
