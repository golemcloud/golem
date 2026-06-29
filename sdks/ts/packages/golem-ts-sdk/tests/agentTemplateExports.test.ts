// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import ts from 'typescript';

import * as sdkEntryPoint from '../src';

function getPath(root: unknown, path: readonly string[]): unknown {
  return path.reduce<unknown>((current, key) => {
    if (typeof current !== 'object' || current === null) return undefined;
    return (current as Record<string, unknown>)[key];
  }, root);
}

describe('agent template export wiring', () => {
  it('SDK entrypoint exports the JS paths the generated WIT wrapper invokes', () => {
    const wrapper = readFileSync(new URL('../agent-template/src/lib.rs', import.meta.url), 'utf8');

    const invokedPaths = [
      ['guest', 'initialize'],
      ['guest', 'invoke'],
      ['guest', 'getDefinition'],
      ['guest', 'discoverAgentTypes'],
      ['guest', 'discoverTools'],
      ['guest', 'getTool'],
      ['guest', 'invokeTool'],
    ] as const;

    for (const path of invokedPaths) {
      expect(wrapper).toContain(`&["${path.join('", "')}"]`);
      expect(getPath(sdkEntryPoint, path), path.join('.')).toBeTypeOf('function');
    }
  });

  it('review repro: packaged agent_guest.wasm uses split guest.invokeTool wiring', () => {
    const wasm = readFileSync(new URL('../wasm/agent_guest.wasm', import.meta.url));

    expect(wasm.includes(Buffer.from('invokeTool'))).toBe(true);
  });

  it('review repro: exported JS functions satisfy the template arity check', () => {
    const wrapper = readFileSync(new URL('../agent-template/src/lib.rs', import.meta.url), 'utf8');

    const invokedPaths = [
      { path: ['guest', 'initialize'], argCount: 3 },
      { path: ['guest', 'invoke'], argCount: 3 },
      { path: ['guest', 'getDefinition'], argCount: 0 },
      { path: ['guest', 'discoverAgentTypes'], argCount: 0 },
      { path: ['guest', 'discoverTools'], argCount: 0 },
      { path: ['guest', 'getTool'], argCount: 1 },
      { path: ['guest', 'invokeTool'], argCount: 5 },
    ] as const;

    for (const { path, argCount } of invokedPaths) {
      expect(wrapper).toContain(`&["${path.join('", "')}"]`);
      const fn = getPath(sdkEntryPoint, path);
      expect(fn, path.join('.')).toBeTypeOf('function');
      expect((fn as Function).length, `${path.join('.')} called with ${argCount} args`).toBe(
        argCount,
      );
    }
  });

  it('agent-guest declaration alias type-checks as a single merged guest namespace', () => {
    const packageRoot = fileURLToPath(new URL('..', import.meta.url));
    const typeFiles = [
      'exports.d.ts',
      'golem_agent_2_0_0_common.d.ts',
      'golem_api_1_5_0_host.d.ts',
      'golem_core_2_0_0_types.d.ts',
      'golem_tool_0_1_0_common.d.ts',
      'wasi_io_0_2_3_streams.d.ts',
      'wasi_io_0_2_3_error.d.ts',
      'wasi_io_0_2_3_poll.d.ts',
      'wasi_clocks_0_2_3_monotonic_clock.d.ts',
    ].map((file) => path.join(packageRoot, 'types', file));
    const virtualFile = path.join(packageRoot, '__agent_guest_typecheck__.ts');
    const virtualSource = `
      import type { guest } from 'agent-guest';
      type Initialize = typeof guest.initialize;
      type AgentInvoke = typeof guest.invoke;
      type ToolInvoke = typeof guest.invokeTool;
      type DiscoverTools = typeof guest.discoverTools;
    `;
    const options: ts.CompilerOptions = {
      noEmit: true,
      strict: true,
      skipLibCheck: false,
      moduleResolution: ts.ModuleResolutionKind.Node10,
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
      types: [],
      lib: ['lib.esnext.d.ts', 'lib.dom.d.ts'],
    };
    const host = ts.createCompilerHost(options);
    const fileExists = host.fileExists.bind(host);
    const readFile = host.readFile.bind(host);
    const getSourceFile = host.getSourceFile.bind(host);

    host.fileExists = (file) => path.resolve(file) === virtualFile || fileExists(file);
    host.readFile = (file) => (path.resolve(file) === virtualFile ? virtualSource : readFile(file));
    host.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) =>
      path.resolve(file) === virtualFile
        ? ts.createSourceFile(file, virtualSource, languageVersion, true)
        : getSourceFile(file, languageVersion, onError, shouldCreateNewSourceFile);

    const program = ts.createProgram([...typeFiles, virtualFile], options, host);
    const diagnostics = ts.getPreEmitDiagnostics(program);
    const formatted = diagnostics.map((diagnostic) => {
      const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
      if (diagnostic.file && diagnostic.start !== undefined) {
        const pos = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
        return `${path.relative(packageRoot, diagnostic.file.fileName)}:${pos.line + 1}:${pos.character + 1} TS${diagnostic.code}: ${message}`;
      }
      return `TS${diagnostic.code}: ${message}`;
    });

    expect(formatted).toEqual([]);
  });

  it('review repro: agent-guest declaration exposes split invokeTool', () => {
    const packageRoot = fileURLToPath(new URL('..', import.meta.url));
    const typeFiles = [
      'exports.d.ts',
      'golem_agent_2_0_0_common.d.ts',
      'golem_api_1_5_0_host.d.ts',
      'golem_core_2_0_0_types.d.ts',
      'golem_tool_0_1_0_common.d.ts',
      'wasi_io_0_2_3_streams.d.ts',
      'wasi_io_0_2_3_error.d.ts',
      'wasi_io_0_2_3_poll.d.ts',
      'wasi_clocks_0_2_3_monotonic_clock.d.ts',
    ].map((file) => path.join(packageRoot, 'types', file));
    const virtualFile = path.join(packageRoot, '__agent_guest_invoke_tool_typecheck__.ts');
    const virtualSource = `
      import type { guest } from 'agent-guest';

      type InvokeTool = typeof guest.invokeTool;
      type ExpectedInvokeTool = (
        toolName: string,
        commandPath: string[],
        input: guest.TypedSchemaValue,
        stdin: guest.InputStream | undefined,
        principal: guest.Principal,
      ) => Promise<guest.InvocationResult>;
      type _Assert = InvokeTool extends ExpectedInvokeTool ? true : never;
    `;
    const options: ts.CompilerOptions = {
      noEmit: true,
      strict: true,
      skipLibCheck: false,
      moduleResolution: ts.ModuleResolutionKind.Node10,
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
      types: [],
      lib: ['lib.esnext.d.ts', 'lib.dom.d.ts'],
    };
    const host = ts.createCompilerHost(options);
    const fileExists = host.fileExists.bind(host);
    const readFile = host.readFile.bind(host);
    const getSourceFile = host.getSourceFile.bind(host);

    host.fileExists = (file) => path.resolve(file) === virtualFile || fileExists(file);
    host.readFile = (file) => (path.resolve(file) === virtualFile ? virtualSource : readFile(file));
    host.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) =>
      path.resolve(file) === virtualFile
        ? ts.createSourceFile(file, virtualSource, languageVersion, true)
        : getSourceFile(file, languageVersion, onError, shouldCreateNewSourceFile);

    const program = ts.createProgram([...typeFiles, virtualFile], options, host);
    const diagnostics = ts.getPreEmitDiagnostics(program);
    const formatted = diagnostics.map((diagnostic) => {
      const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
      if (diagnostic.file && diagnostic.start !== undefined) {
        const pos = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
        return `${path.relative(packageRoot, diagnostic.file.fileName)}:${pos.line + 1}:${pos.character + 1} TS${diagnostic.code}: ${message}`;
      }
      return `TS${diagnostic.code}: ${message}`;
    });

    expect(formatted).toEqual([]);
  });

  it('review repro: getWithConfig rejects optional secret config override fields', () => {
    const packageRoot = fileURLToPath(new URL('..', import.meta.url));
    const virtualFile = path.join(packageRoot, '__get_with_config_secret_typecheck__.ts');
    const virtualSource = `
      import { BaseAgent, Config, Secret } from './src';

      type Cfg = {
        plain: string;
        requiredSecret: Secret<string>;
        optionalSecret?: Secret<string>;
        explicitOptionalSecret: Secret<string> | undefined;
      };

      class A extends BaseAgent {
        constructor(readonly id: string, readonly config: Config<Cfg>) {
          super();
        }
      }

      A.getWithConfig('id', { plain: 'ok' });

      // @ts-expect-error required secret config fields cannot be supplied through RPC overrides
      A.getWithConfig('id', { requiredSecret: new Secret(['requiredSecret'], { kind: 'string', optional: false }) });

      // @ts-expect-error optional secret config fields cannot be supplied through RPC overrides
      A.getWithConfig('id', { optionalSecret: new Secret(['optionalSecret'], { kind: 'string', optional: false }) });

      // @ts-expect-error explicit Secret<T> | undefined config fields cannot be supplied through RPC overrides
      A.getWithConfig('id', { explicitOptionalSecret: new Secret(['explicitOptionalSecret'], { kind: 'string', optional: false }) });
    `;

    const configFile = ts.readConfigFile(path.join(packageRoot, 'tsconfig.json'), ts.sys.readFile);
    expect(configFile.error).toBeUndefined();
    const parsed = ts.parseJsonConfigFileContent(configFile.config, ts.sys, packageRoot);
    const options: ts.CompilerOptions = {
      ...parsed.options,
      noEmit: true,
      skipLibCheck: true,
      types: [],
    };
    const host = ts.createCompilerHost(options);
    const fileExists = host.fileExists.bind(host);
    const readFile = host.readFile.bind(host);
    const getSourceFile = host.getSourceFile.bind(host);

    host.fileExists = (file) => path.resolve(file) === virtualFile || fileExists(file);
    host.readFile = (file) => (path.resolve(file) === virtualFile ? virtualSource : readFile(file));
    host.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) =>
      path.resolve(file) === virtualFile
        ? ts.createSourceFile(file, virtualSource, languageVersion, true)
        : getSourceFile(file, languageVersion, onError, shouldCreateNewSourceFile);

    const program = ts.createProgram([...parsed.fileNames, virtualFile], options, host);
    const diagnostics = ts
      .getPreEmitDiagnostics(program)
      .filter((diagnostic) => diagnostic.file?.fileName === virtualFile);
    const formatted = diagnostics.map((diagnostic) => {
      const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
      if (diagnostic.file && diagnostic.start !== undefined) {
        const pos = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
        return `${path.relative(packageRoot, diagnostic.file.fileName)}:${pos.line + 1}:${pos.character + 1} TS${diagnostic.code}: ${message}`;
      }
      return `TS${diagnostic.code}: ${message}`;
    });

    expect(formatted).toEqual([]);
  });

  it('review repro: getWithConfig rejects nested secret-only config override groups', () => {
    const packageRoot = fileURLToPath(new URL('..', import.meta.url));
    const virtualFile = path.join(packageRoot, '__get_with_config_nested_secret_typecheck__.ts');
    const virtualSource = `
      import { BaseAgent, Config, Secret } from './src';

      type Cfg = {
        auth: {
          apiKey: Secret<string>;
        };
        optionalAuth?: {
          token?: Secret<string>;
        };
      };

      class A extends BaseAgent {
        constructor(readonly config: Config<Cfg>) {
          super();
        }
      }

      A.getWithConfig({});

      // @ts-expect-error nested required secret config fields cannot be supplied through RPC overrides
      A.getWithConfig({ auth: { apiKey: new Secret(['auth', 'apiKey'], { kind: 'string', optional: false }) } });

      // @ts-expect-error nested optional secret config fields cannot be supplied through RPC overrides
      A.getWithConfig({ optionalAuth: { token: new Secret(['optionalAuth', 'token'], { kind: 'string', optional: false }) } });
    `;

    const configFile = ts.readConfigFile(path.join(packageRoot, 'tsconfig.json'), ts.sys.readFile);
    expect(configFile.error).toBeUndefined();
    const parsed = ts.parseJsonConfigFileContent(configFile.config, ts.sys, packageRoot);
    const options: ts.CompilerOptions = {
      ...parsed.options,
      noEmit: true,
      skipLibCheck: true,
      types: [],
    };
    const host = ts.createCompilerHost(options);
    const fileExists = host.fileExists.bind(host);
    const readFile = host.readFile.bind(host);
    const getSourceFile = host.getSourceFile.bind(host);

    host.fileExists = (file) => path.resolve(file) === virtualFile || fileExists(file);
    host.readFile = (file) => (path.resolve(file) === virtualFile ? virtualSource : readFile(file));
    host.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) =>
      path.resolve(file) === virtualFile
        ? ts.createSourceFile(file, virtualSource, languageVersion, true)
        : getSourceFile(file, languageVersion, onError, shouldCreateNewSourceFile);

    const program = ts.createProgram([...parsed.fileNames, virtualFile], options, host);
    const diagnostics = ts
      .getPreEmitDiagnostics(program)
      .filter((diagnostic) => diagnostic.file?.fileName === virtualFile);
    const formatted = diagnostics.map((diagnostic) => {
      const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
      if (diagnostic.file && diagnostic.start !== undefined) {
        const pos = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
        return `${path.relative(packageRoot, diagnostic.file.fileName)}:${pos.line + 1}:${pos.character + 1} TS${diagnostic.code}: ${message}`;
      }
      return `TS${diagnostic.code}: ${message}`;
    });

    expect(formatted).toEqual([]);
  });

  it('review repro: remote method Config parameters are injected and not caller supplied', () => {
    const packageRoot = fileURLToPath(new URL('..', import.meta.url));
    const virtualFile = path.join(packageRoot, '__remote_method_config_typecheck__.ts');
    const virtualSource = `
      import { BaseAgent, Config } from './src';

      type Cfg = { plain: string };

      class A extends BaseAgent {
        async foo(config: Config<Cfg>, input: string): Promise<void> {}
      }

      const client = A.get();
      client.foo('x');

      // @ts-expect-error config parameters are injected, not supplied by remote callers
      client.foo(new Config<Cfg>([], []), 'x');
    `;

    const configFile = ts.readConfigFile(path.join(packageRoot, 'tsconfig.json'), ts.sys.readFile);
    expect(configFile.error).toBeUndefined();
    const parsed = ts.parseJsonConfigFileContent(configFile.config, ts.sys, packageRoot);
    const options: ts.CompilerOptions = {
      ...parsed.options,
      noEmit: true,
      skipLibCheck: true,
      types: [],
    };
    const host = ts.createCompilerHost(options);
    const fileExists = host.fileExists.bind(host);
    const readFile = host.readFile.bind(host);
    const getSourceFile = host.getSourceFile.bind(host);

    host.fileExists = (file) => path.resolve(file) === virtualFile || fileExists(file);
    host.readFile = (file) => (path.resolve(file) === virtualFile ? virtualSource : readFile(file));
    host.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) =>
      path.resolve(file) === virtualFile
        ? ts.createSourceFile(file, virtualSource, languageVersion, true)
        : getSourceFile(file, languageVersion, onError, shouldCreateNewSourceFile);

    const program = ts.createProgram([...parsed.fileNames, virtualFile], options, host);
    const diagnostics = ts
      .getPreEmitDiagnostics(program)
      .filter((diagnostic) => diagnostic.file?.fileName === virtualFile);
    const formatted = diagnostics.map((diagnostic) => {
      const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
      if (diagnostic.file && diagnostic.start !== undefined) {
        const pos = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
        return `${path.relative(packageRoot, diagnostic.file.fileName)}:${pos.line + 1}:${pos.character + 1} TS${diagnostic.code}: ${message}`;
      }
      return `TS${diagnostic.code}: ${message}`;
    });

    expect(formatted).toEqual([]);
  });

  it('review repro: getWithConfig accepts non-secret array config override leaves', () => {
    const packageRoot = fileURLToPath(new URL('..', import.meta.url));
    const virtualFile = path.join(packageRoot, '__get_with_config_array_override_typecheck__.ts');
    const virtualSource = `
      import { BaseAgent, Config } from './src';

      type Cfg = {
        tags: string[];
      };

      class A extends BaseAgent {
        constructor(readonly config: Config<Cfg>) {
          super();
        }
      }

      A.getWithConfig({ tags: ['prod', 'blue'] });
    `;

    const configFile = ts.readConfigFile(path.join(packageRoot, 'tsconfig.json'), ts.sys.readFile);
    expect(configFile.error).toBeUndefined();
    const parsed = ts.parseJsonConfigFileContent(configFile.config, ts.sys, packageRoot);
    const options: ts.CompilerOptions = {
      ...parsed.options,
      noEmit: true,
      skipLibCheck: true,
      types: [],
    };
    const host = ts.createCompilerHost(options);
    const fileExists = host.fileExists.bind(host);
    const readFile = host.readFile.bind(host);
    const getSourceFile = host.getSourceFile.bind(host);

    host.fileExists = (file) => path.resolve(file) === virtualFile || fileExists(file);
    host.readFile = (file) => (path.resolve(file) === virtualFile ? virtualSource : readFile(file));
    host.getSourceFile = (file, languageVersion, onError, shouldCreateNewSourceFile) =>
      path.resolve(file) === virtualFile
        ? ts.createSourceFile(file, virtualSource, languageVersion, true)
        : getSourceFile(file, languageVersion, onError, shouldCreateNewSourceFile);

    const program = ts.createProgram([...parsed.fileNames, virtualFile], options, host);
    const diagnostics = ts
      .getPreEmitDiagnostics(program)
      .filter((diagnostic) => diagnostic.file?.fileName === virtualFile);
    const formatted = diagnostics.map((diagnostic) => {
      const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
      if (diagnostic.file && diagnostic.start !== undefined) {
        const pos = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
        return `${path.relative(packageRoot, diagnostic.file.fileName)}:${pos.line + 1}:${pos.character + 1} TS${diagnostic.code}: ${message}`;
      }
      return `TS${diagnostic.code}: ${message}`;
    });

    expect(formatted).toEqual([]);
  });
});
