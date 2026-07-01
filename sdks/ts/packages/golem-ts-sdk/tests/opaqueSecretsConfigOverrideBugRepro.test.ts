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

import { describe, expect, it } from 'vitest';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

function typecheckVirtualSource(fileName: string, virtualSource: string): string[] {
  const packageRoot = fileURLToPath(new URL('..', import.meta.url));
  const virtualFile = path.join(packageRoot, fileName);
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

  return diagnostics.map((diagnostic) => {
    const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
    if (diagnostic.file && diagnostic.start !== undefined) {
      const pos = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
      return `${path.relative(packageRoot, diagnostic.file.fileName)}:${pos.line + 1}:${pos.character + 1} TS${diagnostic.code}: ${message}`;
    }
    return `TS${diagnostic.code}: ${message}`;
  });
}

describe('opaque secret config override type holes', () => {
  it('rejects root secret-only config override fields', () => {
    const diagnostics = typecheckVirtualSource(
      '__root_secret_only_config_override_typecheck__.ts',
      `
        import { BaseAgent, Config, Secret } from './src';

        type Cfg = {
          apiKey: Secret<string>;
          optionalToken?: Secret<string>;
        };

        class A extends BaseAgent {
          constructor(readonly config: Config<Cfg>) {
            super();
          }
        }

        A.getWithConfig({});

        // @ts-expect-error root secret config fields cannot be supplied through RPC overrides
        A.getWithConfig({ apiKey: new Secret<string>(['apiKey'], { kind: 'string', optional: false }) });
      `,
    );

    expect(diagnostics).toEqual([]);
  });

  it('rejects config override arrays whose elements contain secrets', () => {
    const diagnostics = typecheckVirtualSource(
      '__array_secret_config_override_typecheck__.ts',
      `
        import { BaseAgent, Config, Secret } from './src';

        type Cfg = {
          labels: string[];
          secrets: Secret<string>[];
          entries: { label: string; token: Secret<string> }[];
        };

        class A extends BaseAgent {
          constructor(readonly config: Config<Cfg>) {
            super();
          }
        }

        A.getWithConfig({ labels: ['prod', 'blue'] });

        // @ts-expect-error array elements containing secrets cannot be supplied through RPC overrides
        A.getWithConfig({ secrets: [new Secret<string>(['secrets'], { kind: 'string', optional: false })] });

        // @ts-expect-error nested array object secrets cannot be supplied through RPC overrides
        A.getWithConfig({ entries: [{ label: 'prod', token: new Secret<string>(['entries', 'token'], { kind: 'string', optional: false }) }] });
      `,
    );

    expect(diagnostics).toEqual([]);
  });

  it('rejects explicit undefined for optional secret-bearing override containers', () => {
    const diagnostics = typecheckVirtualSource(
      '__optional_secret_container_undefined_override_typecheck__.ts',
      `
        import { BaseAgent, Config, Secret } from './src';

        type Cfg = {
          labels?: string[];
          secrets?: Secret<string>[];
          entries?: { label: string; token: Secret<string> }[];
          secretValues?: Map<string, Secret<string>>;
        };

        class A extends BaseAgent {
          constructor(readonly config: Config<Cfg>) {
            super();
          }
        }

        A.getWithConfig({ labels: undefined });

        // @ts-expect-error explicit undefined for a secret-bearing array override must be rejected
        A.getWithConfig({ secrets: undefined });

        // @ts-expect-error explicit undefined for a secret-bearing object-array override must be rejected
        A.getWithConfig({ entries: undefined });

        // @ts-expect-error explicit undefined for a secret-bearing map override must be rejected
        A.getWithConfig({ secretValues: undefined });
      `,
    );

    expect(diagnostics).toEqual([]);
  });

  it('accepts non-secret map override leaves while rejecting secret-bearing maps', () => {
    const diagnostics = typecheckVirtualSource(
      '__map_secret_config_override_typecheck__.ts',
      `
        import { BaseAgent, Config, Secret } from './src';

        type Cfg = {
          labels: Map<string, string>;
          secretValues: Map<string, Secret<string>>;
          secretKeys: Map<Secret<string>, string>;
        };

        class A extends BaseAgent {
          constructor(readonly config: Config<Cfg>) {
            super();
          }
        }

        A.getWithConfig({ labels: new Map([['env', 'prod']]) });

        // @ts-expect-error map values containing secrets cannot be supplied through RPC overrides
        A.getWithConfig({ secretValues: new Map([['token', new Secret<string>(['secretValues'], { kind: 'string', optional: false })]]) });

        // @ts-expect-error map keys containing secrets cannot be supplied through RPC overrides
        A.getWithConfig({ secretKeys: new Map([[new Secret<string>(['secretKeys'], { kind: 'string', optional: false }), 'token']]) });
      `,
    );

    expect(diagnostics).toEqual([]);
  });

  it('rejects null overrides for nullable secret-bearing config fields', () => {
    const diagnostics = typecheckVirtualSource(
      '__nullable_secret_config_override_typecheck__.ts',
      `
        import { BaseAgent, Config, Secret } from './src';

        type Cfg = {
          nullableSecret: Secret<string> | null;
          nullableSecretArray: Secret<string>[] | null;
          nullableSecretObject: { token: Secret<string> } | null;
          nullableSecretUnion: string | Secret<string> | null;
        };

        class A extends BaseAgent {
          constructor(readonly config: Config<Cfg>) {
            super();
          }
        }

        // @ts-expect-error nullable secret config fields cannot be supplied through RPC overrides
        A.getWithConfig({ nullableSecret: null });

        // @ts-expect-error nullable secret-bearing arrays cannot be supplied through RPC overrides
        A.getWithConfig({ nullableSecretArray: null });

        // @ts-expect-error nullable secret-bearing objects cannot be supplied through RPC overrides
        A.getWithConfig({ nullableSecretObject: null });

        // @ts-expect-error nullable unions that can carry secrets cannot be supplied through RPC overrides
        A.getWithConfig({ nullableSecretUnion: null });
      `,
    );

    expect(diagnostics).toEqual([]);
  });

  it('rejects safe-looking overrides for unions whose other branches can carry secrets', () => {
    const diagnostics = typecheckVirtualSource(
      '__union_secret_config_override_typecheck__.ts',
      `
        import { BaseAgent, Config, Secret } from './src';

        type SafeOrSecret = { label: string } | { token: Secret<string> };

        type Cfg = {
          mixed: SafeOrSecret;
          entries: SafeOrSecret[];
          values: Map<string, SafeOrSecret>;
        };

        class A extends BaseAgent {
          constructor(readonly config: Config<Cfg>) {
            super();
          }
        }

        // @ts-expect-error union fields that can carry Secret<T> cannot be overridden through RPC config
        A.getWithConfig({ mixed: { label: 'prod' } });

        // @ts-expect-error array elements with a union branch that can carry Secret<T> cannot be overridden
        A.getWithConfig({ entries: [{ label: 'prod' }] });

        // @ts-expect-error map values with a union branch that can carry Secret<T> cannot be overridden
        A.getWithConfig({ values: new Map([['env', { label: 'prod' }]]) });
      `,
    );

    expect(diagnostics).toEqual([]);
  });
});
