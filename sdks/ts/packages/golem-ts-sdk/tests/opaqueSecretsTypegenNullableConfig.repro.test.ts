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
import { Project } from 'ts-morph';
import { buildJSONFromType, buildTypeFromJSON } from '@golemcloud/golem-ts-types-core';
import { getTypeFromTsMorph } from '../../golem-ts-typegen/src/index.js';
import { createWellKnownTypes } from '../../golem-ts-typegen/src/wellknownTypes.js';
import { resolveAgentConfig } from '../src/internal/schema/agentType';
import * as Either from '../src/newTypes/either';
import { normalizeSchema } from './agentTypeHelpers';

describe('opaque secret typegen config adversarial repros', () => {
  it('classifies nullable Secret<T> config members as secret config entries', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__nullable_secret_config_repro__.ts',
      `
        import { Config, Secret } from './src';

        class Test {
          config!: Config<{
            apiKey: Secret<string> | null;
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    expect(configType.properties).toEqual([
      {
        path: ['apiKey'],
        secret: true,
        secretHandleOptional: false,
        type: { kind: 'string', name: undefined, owner: undefined, optional: true },
      },
    ]);
  });

  it('keeps nullable Secret<T> config members required', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__required_nullable_secret_config_repro__.ts',
      `
        import { Config, Secret } from './src';

        class Test {
          config!: Config<{
            apiKey: Secret<string> | null;
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    expect(configType.requiredMembers).toEqual([{ path: [], requiredKeys: ['apiKey'] }]);
  });

  it('review repro: preserves required non-secret siblings in secret-bearing config groups', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__required_secret_group_sibling_config_repro__.ts',
      `
        import { Config, Secret } from './src';

        class Test {
          config!: Config<{
            group: {
              apiKey: Secret<string>;
              label: string;
            };
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    expect(configType.requiredMembers).toEqual([
      { path: ['group'], requiredKeys: ['apiKey', 'label'] },
      { path: [], requiredKeys: ['group'] },
    ]);
  });

  it('review repro: treats Secret<T> | null | undefined config members as optional handles', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__nullish_secret_config_repro__.ts',
      `
        import { Config, Secret } from './src';

        class Test {
          config!: Config<{
            apiKey: Secret<string> | null | undefined;
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    expect(configType.properties).toEqual([
      {
        path: ['apiKey'],
        secret: true,
        secretHandleOptional: true,
        type: { kind: 'string', name: undefined, owner: undefined, optional: true },
      },
    ]);
    expect(configType.requiredMembers).toEqual([]);
  });

  it('review repro: serialized metadata preserves optional secret handles separately from nullable payloads', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__serialized_optional_secret_config_repro__.ts',
      `
        import { Config, Secret } from './src';

        class Test {
          config!: Config<{
            apiKey: Secret<string> | undefined;
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;
    expect(configType.properties[0]).toMatchObject({
      path: ['apiKey'],
      secret: true,
      secretHandleOptional: true,
      type: { kind: 'string', optional: false },
    });

    const serialized = buildJSONFromType(configType);
    const rehydrated = buildTypeFromJSON(serialized);

    const resolved = resolveAgentConfig([{ name: 'config', type: rehydrated }]);
    expect(Either.isRight(resolved)).toBe(true);
    if (!Either.isRight(resolved)) return;

    expect(
      normalizeSchema(resolved.val[0].valueGraph.root, resolved.val[0].valueGraph.defs),
    ).toEqual({
      option: { secret: 'string' },
    });
  });

  it('review repro: preserves nullable payloads separately from optional secret handles in agent config schema', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__optional_nullable_secret_schema_repro__.ts',
      `
        import { Config, Secret } from './src';

        class Test {
          config!: Config<{
            apiKey: Secret<string> | null | undefined;
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    const resolved = resolveAgentConfig([{ name: 'config', type: configType }]);
    expect(Either.isRight(resolved)).toBe(true);
    if (!Either.isRight(resolved)) return;

    expect(resolved.val).toHaveLength(1);
    const graph = resolved.val[0].valueGraph;
    expect(normalizeSchema(graph.root, graph.defs)).toEqual({
      option: { secret: { option: 'string' } },
    });
  });

  it('review repro: keeps nested Secret<T> | null config handles required in agent config schema', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__nested_nullable_secret_config_repro__.ts',
      `
        import { Config, Secret } from './src';

        class Test {
          config!: Config<{
            group: {
              apiKey: Secret<string> | null;
            };
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    const resolved = resolveAgentConfig([{ name: 'config', type: configType }]);
    expect(Either.isRight(resolved)).toBe(true);
    if (!Either.isRight(resolved)) return;

    expect(resolved.val).toHaveLength(1);
    const graph = resolved.val[0].valueGraph;
    expect(normalizeSchema(graph.root, graph.defs)).toEqual({ secret: { option: 'string' } });
  });

  it('preserves null in optional nullable non-secret config members', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__optional_nullable_config_repro__.ts',
      `
        import { Config } from './src';

        class Test {
          config!: Config<{
            optionalLabel?: string | null;
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    expect(configType.properties[0].path).toEqual(['optionalLabel']);
    expect(configType.properties[0].secret).toBe(false);
    expect(configType.properties[0].type.kind).toBe('union');
    if (configType.properties[0].type.kind !== 'union') return;
    expect(configType.properties[0].type.optional).toBe(true);
    expect(configType.properties[0].type.unionTypes.map((type) => type.kind)).toContain('null');
  });

  it('review repro: preserves required non-secret siblings when secret sibling handle is optional', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__optional_secret_required_sibling_config_repro__.ts',
      `
        import { Config, Secret } from './src';

        class Test {
          config!: Config<{
            group: {
              maybeApiKey: Secret<string> | undefined;
              endpoint: string;
            };
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    expect(configType.properties).toEqual(
      expect.arrayContaining([
        {
          path: ['group', 'maybeApiKey'],
          secret: true,
          secretHandleOptional: true,
          type: { kind: 'string', name: undefined, owner: undefined, optional: false },
        },
        {
          path: ['group', 'endpoint'],
          secret: false,
          type: { kind: 'string', name: undefined, owner: undefined, optional: false },
        },
      ]),
    );
    expect(configType.requiredMembers).toEqual([{ path: ['group'], requiredKeys: ['endpoint'] }]);
  });

  it('review repro: preserves required metadata for nested non-secret config groups', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__nested_non_secret_required_group_config_repro__.ts',
      `
        import { Config } from './src';

        class Test {
          config!: Config<{
            group: {
              endpoint: string;
              retries?: number;
            };
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    expect(configType.properties).toEqual(
      expect.arrayContaining([
        {
          path: ['group', 'endpoint'],
          secret: false,
          type: { kind: 'string', name: undefined, owner: undefined, optional: false },
        },
        {
          path: ['group', 'retries'],
          secret: false,
          type: { kind: 'number', name: undefined, owner: undefined, optional: true },
        },
      ]),
    );
    expect(configType.requiredMembers).toEqual([{ path: ['group'], requiredKeys: ['endpoint'] }]);
  });

  it('review repro: optional parent groups do not make required secret payloads nullable', () => {
    const project = new Project({ tsConfigFilePath: 'tsconfig.json' });
    const wellKnown = createWellKnownTypes(project, './src');
    const sf = project.createSourceFile(
      '__optional_parent_required_secret_payload_repro__.ts',
      `
        import { Config, Secret } from './src';

        class Test {
          config!: Config<{
            group?: {
              apiKey: Secret<string>;
            };
          }>;
        }
      `,
      { overwrite: true },
    );

    const configProp = sf.getClassOrThrow('Test').getInstancePropertyOrThrow('config');
    const configType = getTypeFromTsMorph(configProp.getType(), false, wellKnown);

    expect(configType.kind).toBe('config');
    if (configType.kind !== 'config') return;

    expect(configType.properties).toEqual([
      {
        path: ['group', 'apiKey'],
        secret: true,
        secretHandleOptional: true,
        type: { kind: 'string', name: undefined, owner: undefined, optional: false },
      },
    ]);

    const resolved = resolveAgentConfig([{ name: 'config', type: configType }]);
    expect(Either.isRight(resolved)).toBe(true);
    if (!Either.isRight(resolved)) return;

    expect(
      normalizeSchema(resolved.val[0].valueGraph.root, resolved.val[0].valueGraph.defs),
    ).toEqual({
      option: { secret: 'string' },
    });
  });
});
