import { expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import ts from 'typescript';
import { discoverCapabilities } from '../scripts/component.mjs';

it('retains a builder passed to an ambient opaque function', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'golem-capability-'));
  try {
    const main = path.join(dir, 'main.ts');
    fs.writeFileSync(
      main,
      'import { toolDefinition } from "@golemcloud/golem-ts-sdk"; declare function install(builder: ReturnType<typeof toolDefinition>): void; install(toolDefinition("hidden"));',
    );

    expect(
      discoverCapabilities({
        fileNames: [main],
        options: {
          module: ts.ModuleKind.ESNext,
          moduleResolution: ts.ModuleResolutionKind.Bundler,
          target: ts.ScriptTarget.ES2022,
          paths: { '@golemcloud/golem-ts-sdk': [path.resolve('dist/index.d.mts')] },
        },
      }),
    ).toEqual({ agents: false, tools: true, middleware: true });
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

it.each(['inline-import', 'package-dependency'])(
  'retains capabilities for opaque third-party code: %s',
  (kind) => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'golem-capability-'));
    try {
      const dependency = path.join(dir, 'node_modules', 'opaque-registration');
      fs.mkdirSync(dependency, { recursive: true });
      fs.writeFileSync(
        path.join(dependency, 'package.json'),
        JSON.stringify({
          name: 'opaque-registration',
          types: 'index.d.ts',
          main: 'index.js',
          dependencies: kind === 'package-dependency' ? { '@golemcloud/golem-ts-sdk': '*' } : {},
        }),
      );
      fs.writeFileSync(
        path.join(dependency, 'index.d.ts'),
        kind === 'inline-import'
          ? 'export declare function install(builder: ReturnType<typeof import("@golemcloud/golem-ts-sdk").toolDefinition>): void;'
          : 'export declare function install(): void;',
      );
      fs.writeFileSync(
        path.join(dependency, 'index.js'),
        kind === 'inline-import'
          ? 'exports.install = builder => builder.implement({});'
          : 'const { toolDefinition } = require("@golemcloud/golem-ts-sdk"); exports.install = () => toolDefinition("hidden").implement({});',
      );
      const main = path.join(dir, 'main.ts');
      fs.writeFileSync(
        main,
        kind === 'inline-import'
          ? 'import { toolDefinition } from "@golemcloud/golem-ts-sdk"; import { install } from "opaque-registration"; install(toolDefinition("hidden"));'
          : 'import { install } from "opaque-registration"; install();',
      );

      expect(
        discoverCapabilities({
          fileNames: [main],
          options: {
            module: ts.ModuleKind.ESNext,
            moduleResolution: ts.ModuleResolutionKind.Bundler,
            target: ts.ScriptTarget.ES2022,
            paths: { '@golemcloud/golem-ts-sdk': [path.resolve('dist/index.d.mts')] },
          },
        }),
      ).toEqual({ agents: true, tools: true, middleware: true });
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  },
);
