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

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it, vi } from 'vitest';
import ts from 'typescript';
import { deserializeGraph, getGraphCodec } from '../src/internal/mapping/values/schemaValue';
import { r, resolvedField } from '../src/internal/mapping/types/resolvedType';
import { schemaValueFromWit, schemaValueToWit } from '../src/internal/schema-model';

function sdkSourceDiagnostics(): string[] {
  const packageRoot = fileURLToPath(new URL('..', import.meta.url));
  const configFile = ts.readConfigFile(path.join(packageRoot, 'tsconfig.json'), ts.sys.readFile);
  if (configFile.error) {
    return [ts.flattenDiagnosticMessageText(configFile.error.messageText, '\n')];
  }

  const parsed = ts.parseJsonConfigFileContent(configFile.config, ts.sys, packageRoot);
  const sourceFiles = parsed.fileNames.filter((file) =>
    path.relative(packageRoot, file).startsWith(`src${path.sep}`),
  );
  const typeFiles = parsed.fileNames.filter((file) =>
    path.relative(packageRoot, file).startsWith(`types${path.sep}`),
  );
  const program = ts.createProgram([...sourceFiles, ...typeFiles], {
    ...parsed.options,
    noEmit: true,
    skipLibCheck: true,
    types: [],
  });

  return ts.getPreEmitDiagnostics(program).map((diagnostic) => {
    const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n');
    if (diagnostic.file && diagnostic.start !== undefined) {
      const pos = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
      return `${path.relative(packageRoot, diagnostic.file.fileName)}:${pos.line + 1}:${pos.character + 1} TS${diagnostic.code}: ${message}`;
    }
    return `TS${diagnostic.code}: ${message}`;
  });
}

describe('opaque secrets review repros', () => {
  afterEach(() => {
    vi.doUnmock('golem:agent/host@2.0.0');
    vi.doUnmock('golem:secrets/reveal@0.1.0');
    vi.resetModules();
  });

  it('review repro: initiateFromWit source type-checks against ResolvedAgent constructor', () => {
    const diagnostics = sdkSourceDiagnostics().filter((diagnostic) =>
      diagnostic.startsWith('src/decorators/agent.ts:'),
    );

    expect(diagnostics).toEqual([]);
  });

  it('review repro: getWithConfig secret detection source type-checks under strict null checks', () => {
    const diagnostics = sdkSourceDiagnostics().filter((diagnostic) =>
      diagnostic.startsWith('src/internal/clientGeneration.ts:'),
    );

    expect(diagnostics).toEqual([]);
  });

  it('compiled codec read failure leaves the shared cycle guard poisoned', () => {
    const nodes = [
      { tag: 'record-value', val: [1] },
      { tag: 'string-value', val: 'not-a-u32' },
    ] as Parameters<NonNullable<ReturnType<typeof getGraphCodec>>['read']>[1];
    const graph = { defs: new Map(), root: r.record([resolvedField('x', r.u32())]) };
    const codec = getGraphCodec(graph);
    expect(codec).not.toBeNull();

    const onPath = new Uint8Array(nodes.length);

    expect(() => codec!.read(0, nodes, onPath)).toThrow(/number/);
    expect(onPath[0]).toBe(0);
    expect(() => codec!.read(0, nodes, onPath)).toThrow(/number/);
  });

  it('malformed path-backed secret reveal consumes the config-owned handle', async () => {
    const rawSecret = { id: 'config-secret' } as never;
    const configValue = {
      valueNodes: [{ tag: 'secret-value', val: rawSecret }],
      root: 0,
    };
    const getConfigValue = vi.fn(() => configValue);
    const reveal = vi.fn(() => ({
      valueNodes: [{ tag: 'u32-value', val: 7 }],
      root: 0,
    }));

    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal }));

    const { Secret } = await import('../src/agentConfig');
    const secret = new Secret(['apiKey'], { kind: 'string', optional: false });

    expect(() => secret.get()).toThrow(/string/);
    expect((configValue.valueNodes[0] as { val: unknown }).val).toBe(rawSecret);
  });

  it('schemaValueFromWit failure preserves unreferenced secret handles', () => {
    const rawSecret = { id: 'unreferenced-secret' } as never;
    const wit = {
      valueNodes: [
        { tag: 'string-value', val: 'ok' },
        { tag: 'secret-value', val: rawSecret },
      ],
      root: 0,
    } as Parameters<typeof schemaValueFromWit>[0];

    expect(() => schemaValueFromWit(wit)).toThrow(/secret handle not referenced/);
    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(rawSecret);
  });

  it('schemaValueToWit rejects forged secret handle-like schema values', () => {
    const rawSecret = { id: 'forged-secret' } as never;
    const forgedHandle = {
      withHandle: (f: (raw: unknown) => unknown) => f(rawSecret),
      take: () => rawSecret,
    };

    expect(() => schemaValueToWit({ tag: 'secret', handle: forgedHandle as never })).toThrow();
  });

  it('review repro: schema-value secret decode rejects forged handle-like objects', () => {
    const rawSecret = { id: 'forged-low-level-secret' } as never;
    const forgedHandle = {
      withHandle: (f: (raw: unknown) => unknown) => f(rawSecret),
      take: () => rawSecret,
    };

    expect(() =>
      deserializeGraph(
        { tag: 'secret', handle: forgedHandle as never },
        { defs: new Map(), root: r.secret(r.string()) },
      ),
    ).toThrow();
  });

  it('schemaValueFromWit failure after lifting a secret preserves the caller-owned handle', () => {
    const rawSecret = { id: 'secret-before-malformed-node' } as never;
    const wit = {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] },
        { tag: 'secret-value', val: rawSecret },
        { tag: 'duration-value', val: undefined },
      ],
      root: 0,
    } as Parameters<typeof schemaValueFromWit>[0];

    expect(() => schemaValueFromWit(wit)).toThrow();
    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(rawSecret);
  });

  it('secret config preflight failure drains unexpected quota-token handles', async () => {
    const rawSecret = { id: 'config-secret' } as never;
    const rawQuota = { id: 'unexpected-quota-token' } as never;
    const configValue = {
      valueNodes: [
        { tag: 'secret-value', val: rawSecret },
        { tag: 'quota-token-handle', val: rawQuota },
      ],
      root: 0,
    };

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue: vi.fn(() => configValue) }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({
      reveal: () => {
        throw new Error('reveal should not be called for a malformed config value tree');
      },
    }));

    const { Secret } = await import('../src/agentConfig');
    const secret = new Secret(['apiKey'], { kind: 'string', optional: false });

    expect(() => secret.get()).toThrow(/quota-token handle not referenced/);
    expect((configValue.valueNodes[1] as { val: unknown }).val).toBeUndefined();
  });

  it('review repro: materializing an optional secret config entry does not transfer the handle before use', async () => {
    const rawSecret = { id: 'optional-config-secret' } as never;
    const configValue = {
      valueNodes: [
        { tag: 'secret-value', val: rawSecret },
        { tag: 'option-value', val: 0 },
      ],
      root: 1,
    };

    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({ getConfigValue: vi.fn(() => configValue) }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({
      reveal: () => {
        throw new Error('reveal should not be called while only materializing Config.value');
      },
    }));

    const { Config, Secret } = await import('../src/agentConfig');
    const config = new Config(
      [
        {
          path: ['optionalSecret'],
          secret: true,
          type: { kind: 'string', optional: true },
        },
      ],
      [],
    );

    expect((config.value as { optionalSecret?: unknown }).optionalSecret).toBeInstanceOf(Secret);
    expect((configValue.valueNodes[0] as { val: unknown }).val).toBe(rawSecret);
  });

  it('review repro: initiateFromWit constructor decode failure preserves a caller-owned secret handle', async () => {
    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({
      makeAgentId: vi.fn(),
      parseAgentId: vi.fn(),
      getConfigValue: vi.fn(),
      Datetime: class {},
      WasmRpc: vi.fn(),
    }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn() }));

    const { TypeMetadata } = await import('@golemcloud/golem-ts-types-core');
    const { agent } = await import('../src/decorators/agent');
    const { BaseAgent } = await import('../src/baseAgent');
    const { AgentInitiatorRegistry } =
      await import('../src/internal/registry/agentInitiatorRegistry');

    const agentName = 'InitiateFromWitSecretFailureAgent';
    const AgentClass = class extends BaseAgent {
      constructor(
        readonly secret: unknown,
        readonly value: number,
      ) {
        super();
      }
    };
    Object.defineProperty(AgentClass, 'name', { value: agentName });

    const stringType = { kind: 'string' as const, optional: false };
    const secretType = {
      kind: 'secret' as const,
      optional: false,
      element: stringType,
    };
    TypeMetadata.update(
      agentName,
      [
        { name: 'secret', type: secretType },
        { name: 'value', type: { kind: 'number' as const, optional: false } },
      ],
      new Map(),
    );
    agent({ name: agentName })(AgentClass);

    const rawSecret = { id: 'constructor-secret-before-bad-field' } as never;
    const constructorInput = {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] },
        { tag: 'secret-value', val: rawSecret },
        { tag: 'string-value', val: 'not-a-number' },
      ],
      root: 0,
    } as Parameters<
      NonNullable<ReturnType<typeof AgentInitiatorRegistry.lookup>['initiateFromWit']>
    >[0];

    const initiator = AgentInitiatorRegistry.lookup(agentName)!;
    expect(initiator.initiateFromWit).toBeDefined();

    const result = initiator.initiateFromWit!(constructorInput, {} as never);

    expect(result.tag).toBe('err');
    expect((constructorInput.valueNodes[1] as { val: unknown }).val).toBe(rawSecret);
  });

  it('review repro: initiateFromWit rejects a wrong self agent id before consuming constructor secrets', async () => {
    vi.resetModules();
    vi.doMock('golem:agent/host@2.0.0', () => ({
      makeAgentId: vi.fn(),
      parseAgentId: vi.fn(),
      getConfigValue: vi.fn(),
      Datetime: class {},
      WasmRpc: vi.fn(),
    }));
    vi.doMock('golem:secrets/reveal@0.1.0', () => ({ reveal: vi.fn() }));

    const { TypeMetadata } = await import('@golemcloud/golem-ts-types-core');
    const { agent } = await import('../src/decorators/agent');
    const { BaseAgent } = await import('../src/baseAgent');
    const { AgentInitiatorRegistry } =
      await import('../src/internal/registry/agentInitiatorRegistry');

    const className = 'InitiateFromWitWrongSelfIdSecretAgentClass';
    const agentName = 'ExpectedInitiateFromWitWrongSelfIdSecretAgent';
    const AgentClass = class extends BaseAgent {
      constructor(readonly secret: unknown) {
        super();
      }
    };
    Object.defineProperty(AgentClass, 'name', { value: className });

    TypeMetadata.update(
      className,
      [
        {
          name: 'secret',
          type: {
            kind: 'secret' as const,
            optional: false,
            element: { kind: 'string' as const, optional: false },
          },
        },
      ],
      new Map(),
    );
    agent({ name: agentName })(AgentClass);

    const rawSecret = { id: 'constructor-secret-before-wrong-self-id' } as never;
    const constructorInput = {
      valueNodes: [
        { tag: 'record-value', val: [1] },
        { tag: 'secret-value', val: rawSecret },
      ],
      root: 0,
    } as Parameters<
      NonNullable<ReturnType<typeof AgentInitiatorRegistry.lookup>['initiateFromWit']>
    >[0];

    const previousAgentId = (globalThis as any).currentAgentId;
    (globalThis as any).currentAgentId = 'DifferentAgent(secret-constructor)';

    const initiator = AgentInitiatorRegistry.lookup(agentName)!;
    expect(initiator.initiateFromWit).toBeDefined();

    const result = initiator.initiateFromWit!(constructorInput, {} as never);
    (globalThis as any).currentAgentId = previousAgentId;

    expect({
      resultTag: result.tag,
      secretStillOwnedByCaller:
        (constructorInput.valueNodes[1] as { val: unknown }).val === rawSecret,
    }).toEqual({
      resultTag: 'err',
      secretStillOwnedByCaller: true,
    });
  });
});
