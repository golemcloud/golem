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

import { type FlagSpec } from 'golem:tool/common@0.1.0';
import { describe, expect, it } from 'vitest';
import { z } from 'zod/v4';
import { compileSchema } from '../src/fluent/schema/adapter';
import type { FluentCodec } from '../src/fluent/schema/codec';
import { Bytes, KeyValue, Quantity, s } from '../src/fluent/schema/markers';
import { field, schemaType, t, v, validateSchemaGraph } from '../src/internal/schema-model';
import {
  type ExtendedCommandBody,
  type ExtendedCommandNode,
  type ExtendedOptionSpec,
  ExtendedToolType,
  codecValue,
  emptyDoc,
  emptyGlobals,
  graftSubtree,
  normalizeExtendedTool,
  schemaValueConforms,
  validateExtendedTool,
} from '../src/internal/tool';

const stringCodec = compileSchema(z.string());

function option(long: string, codec: FluentCodec = stringCodec): ExtendedOptionSpec {
  return {
    long,
    aliases: [],
    doc: emptyDoc(),
    shape: { tag: 'scalar', codec },
    required: true,
  };
}

function flag(long: string): FlagSpec {
  return {
    long,
    aliases: [],
    doc: emptyDoc(),
    shape: { tag: 'bool-flag', val: { default_: false, negatable: false } },
  };
}

function body(overrides: Partial<ExtendedCommandBody> = {}): ExtendedCommandBody {
  return {
    positionals: { fixed: [] },
    options: [],
    flags: [],
    constraints: [],
    errors: [],
    ...overrides,
  };
}

function command(name: string, overrides: Partial<ExtendedCommandNode> = {}): ExtendedCommandNode {
  return {
    name,
    aliases: [],
    doc: emptyDoc(),
    globals: emptyGlobals(),
    subcommands: [],
    ...overrides,
  };
}

describe('internal extended tool model', () => {
  it('resolves aliases and builds canonical input in observable field order', () => {
    const leaf = command('search', {
      aliases: ['s'],
      globals: { options: [option('endpoint')], flags: [] },
      body: body({
        positionals: {
          fixed: [
            {
              name: 'source',
              doc: emptyDoc(),
              codec: stringCodec,
              required: true,
              acceptsStdio: false,
            },
          ],
          tail: {
            name: 'patterns',
            doc: emptyDoc(),
            itemCodec: stringCodec,
            min: 0,
            verbatim: false,
            acceptsStdio: false,
          },
        },
        options: [option('format')],
        flags: [flag('dry-run')],
      }),
    });
    const tool = new ExtendedToolType(
      '1.0.0',
      command('grep', {
        globals: { options: [option('profile')], flags: [flag('verbose')] },
        subcommands: [leaf],
      }),
    );

    expect(tool.commandByPath(['s'])).toBe(leaf);
    expect(tool.commandPath(leaf)).toEqual(['search']);
    const input = tool.canonicalInputModel(leaf);
    expect(input.fields.map((field) => field.name)).toEqual([
      'profile',
      'verbose',
      'endpoint',
      'source',
      'patterns',
      'format',
      'dry-run',
    ]);
    const value = {
      profile: 'prod',
      verbose: true,
      endpoint: 'local',
      source: 'src',
      patterns: ['TODO', 'FIXME'],
      format: 'json',
      'dry-run': false,
    };
    expect(input.decode(input.encode(value))).toEqual(value);
    expect(tool.projectHelp(['search'])?.arguments.map((entry) => entry.kind)).toEqual([
      'global-option',
      'global-flag',
      'global-option',
      'positional',
      'tail',
      'option',
      'flag',
    ]);
  });

  it('resolves deferred value-is literals after inherited globals are composed', () => {
    const leaf = command('show', {
      body: body({
        constraints: [
          {
            tag: 'requires-all',
            refs: [
              {
                tag: 'value-is',
                name: 'profile',
                value: { tag: 'deferred', value: 'prod' },
              },
            ],
          },
        ],
      }),
    });
    const source = new ExtendedToolType(
      '1.0.0',
      command('git', {
        globals: { options: [option('profile')], flags: [] },
        subcommands: [leaf],
      }),
    );

    const normalized = normalizeExtendedTool(source);
    const ref = normalized.commandByPath(['show'])?.body?.constraints[0];
    expect(ref?.tag).toBe('requires-all');
    if (ref?.tag !== 'requires-all') throw new Error('unexpected constraint');
    expect(ref.refs[0]).toMatchObject({
      tag: 'value-is',
      name: 'profile',
      value: { tag: 'resolved', value: 'prod', schemaValue: { tag: 'string', value: 'prod' } },
    });
    expect(leaf.body?.constraints[0]).toMatchObject({
      refs: [{ value: { tag: 'deferred' } }],
    });
  });

  it('grafts detached child roots and rejects producer collisions', () => {
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        globals: { options: [option('child-profile')], flags: [] },
        body: body(),
      }),
    );
    const graft = graftSubtree(child, {
      expectedName: 'remote',
      parentGlobals: { options: [option('parent-profile')], flags: [] },
      name: 'origin',
    });
    expect(graft.name).toBe('origin');
    expect(graft.globals.options.map((entry) => entry.long)).toEqual([
      'parent-profile',
      'child-profile',
    ]);
    expect(graft).not.toBe(child.root);

    const invalid = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        globals: { options: [option('profile')], flags: [] },
        body: body({
          positionals: {
            fixed: [
              {
                name: 'profile',
                doc: emptyDoc(),
                codec: stringCodec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );
    expect(() => validateExtendedTool(invalid)).toThrowError(
      expect.objectContaining({ code: 'duplicate-name' }),
    );
  });

  it('accepts a valid f32 restriction spanning negative and positive values', () => {
    const codec = compileSchema(s.f32({ min: -1, max: 1 }));
    const tool = new ExtendedToolType(
      '1.0.0',
      command('numeric', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'value',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).not.toThrow();
  });

  it('accepts negative durations representable by the signed s64 WIT type', () => {
    const codec = compileSchema(s.duration());

    expect(schemaValueConforms(codec.graph, codec.graph.root, v.duration(-1n))).toBe(true);
  });

  it('rejects durations above the signed s64 WIT maximum', () => {
    const codec = compileSchema(s.duration());

    expect(schemaValueConforms(codec.graph, codec.graph.root, v.duration(2n ** 63n))).toBe(false);
  });

  it('validates f32 restrictions after rounding values to their WIT representation', () => {
    const data = new DataView(new ArrayBuffer(8));
    const minimum = Math.fround(0.1);
    data.setFloat64(0, minimum);
    const root = t.f32({ min: { tag: 'float-bits', val: data.getBigUint64(0) } });
    const graph = { defs: new Map(), root };

    expect(validateSchemaGraph(graph)).toEqual([]);
    expect(schemaValueConforms(graph, root, v.f32(0.1))).toBe(true);
  });

  it('treats a secret as opaque when checking for variants in input position', () => {
    const codec: FluentCodec = {
      graph: {
        defs: new Map(),
        root: t.secret(
          t.variant([
            { name: 'token', payload: t.string(), metadata: { aliases: [], examples: [] } },
          ]),
        ),
      },
      toValue: () => {
        throw new Error('not needed by this validation test');
      },
      fromValue: () => {
        throw new Error('not needed by this validation test');
      },
    };
    const tool = new ExtendedToolType(
      '1.0.0',
      command('secret-tool', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'credential',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).not.toThrow();
  });

  it('rejects defaults that violate numeric schema restrictions', () => {
    const codec = compileSchema(s.u8({ min: 10 }));
    const tool = new ExtendedToolType(
      '1.0.0',
      command('numeric', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'count',
                doc: emptyDoc(),
                codec,
                default: codecValue(codec, 1),
                required: false,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'default-type-mismatch' }),
    );
  });

  it('rejects defaults that violate their source literal schema', () => {
    const codec = compileSchema(z.literal('right'));
    const tool = new ExtendedToolType(
      '1.0.0',
      command('literal', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'mode',
                doc: emptyDoc(),
                codec,
                default: codecValue(codec, 'wrong'),
                required: false,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'default-type-mismatch' }),
    );
  });

  it('rejects value-is literals that violate their source literal schema', () => {
    const codec = compileSchema(z.literal('right'));
    const tool = new ExtendedToolType(
      '1.0.0',
      command('literal', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'mode',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
          constraints: [
            {
              tag: 'requires-all',
              refs: [
                {
                  tag: 'value-is',
                  name: 'mode',
                  value: { tag: 'deferred', value: 'wrong' },
                },
              ],
            },
          ],
        }),
      }),
    );

    expect(() => normalizeExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'value-is-type-mismatch' }),
    );
  });

  it('resolves value-is literals from transformed KeyValue outputs', () => {
    const codec = compileSchema(KeyValue(z.coerce.number()));
    const tool = new ExtendedToolType(
      '1.0.0',
      command('lookup', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'entries',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
          constraints: [
            {
              tag: 'requires-all',
              refs: [
                {
                  tag: 'value-is',
                  name: 'entries',
                  value: { tag: 'deferred', value: new Map([['count', '42']]) },
                },
              ],
            },
          ],
        }),
      }),
    );

    const normalized = normalizeExtendedTool(tool);
    const constraint = normalized.root.body?.constraints[0];
    if (constraint?.tag !== 'requires-all') throw new Error('unexpected constraint');
    const ref = constraint.refs[0];
    if (ref.tag !== 'value-is' || ref.value.tag !== 'resolved') {
      throw new Error('value-is was not resolved');
    }
    expect.soft(ref.value.value).toEqual(new Map([['count', 42]]));
    expect
      .soft(ref.value.schemaValue)
      .toEqual(v.map([{ key: v.string('count'), value: v.f64(42) }]));
  });

  it.each([
    ['u8', compileSchema(s.u8({ max: 300 }))],
    ['s8', compileSchema(s.s8({ min: -129 }))],
    ['f32', compileSchema(s.f32({ min: 0.1 }))],
  ])('rejects a numeric restriction not representable by %s', (_name, codec) => {
    const tool = new ExtendedToolType(
      '1.0.0',
      command('numeric', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'value',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'ill-formed-schema' }),
    );
  });

  it('rejects a pure recursive schema alias', () => {
    let alias!: z.ZodType<unknown>;
    alias = z.lazy(() => alias);
    const codec = compileSchema(alias);
    const tool = new ExtendedToolType(
      '1.0.0',
      command('recursive', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'value',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'ill-formed-schema' }),
    );
  });

  it('rejects nested nullable option schemas that collapse distinct values', () => {
    const codec = compileSchema(z.string().nullable().optional());
    const tool = new ExtendedToolType(
      '1.0.0',
      command('nullable', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'value',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'ill-formed-schema' }),
    );
  });

  it('allows value-is to compare one element of a recursive list root', () => {
    let items!: z.ZodType<unknown>;
    const recursiveItems = z.lazy(() => items);
    const item = z.object({ children: recursiveItems });
    items = z.array(item);
    const codec = compileSchema(recursiveItems);
    const tool = new ExtendedToolType(
      '1.0.0',
      command('recursive', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'items',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
          constraints: [
            {
              tag: 'requires-all',
              refs: [
                {
                  tag: 'value-is',
                  name: 'items',
                  value: { tag: 'deferred', value: { children: [] } },
                },
              ],
            },
          ],
        }),
      }),
    );

    expect(() => normalizeExtendedTool(tool)).not.toThrow();
  });

  it('allows value-is to compare one element of a list-valued positional', () => {
    const codec = compileSchema(z.array(z.string()));
    const tool = new ExtendedToolType(
      '1.0.0',
      command('search', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'items',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
          constraints: [
            {
              tag: 'requires-all',
              refs: [
                {
                  tag: 'value-is',
                  name: 'items',
                  value: { tag: 'deferred', value: 'needle' },
                },
              ],
            },
          ],
        }),
      }),
    );

    expect(() => normalizeExtendedTool(tool)).not.toThrow();
  });

  it('rejects non-primitive map keys during producer validation', () => {
    const codec = compileSchema(
      KeyValue(z.string(), {
        keySchema: z.object({ id: z.string() }),
      }),
    );
    const tool = new ExtendedToolType(
      '1.0.0',
      command('lookup', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'entries',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'ill-formed-schema' }),
    );
  });

  it('rejects ambiguous canonical input models from multiple-parent command trees', () => {
    const leaf = command('leaf', { body: body() });
    const fromA = command('a', {
      globals: { options: [option('from-a')], flags: [] },
      subcommands: [leaf],
    });
    const fromB = command('b', {
      globals: { options: [option('from-b')], flags: [] },
      subcommands: [leaf],
    });
    const tool = new ExtendedToolType('1.0.0', command('root', { subcommands: [fromA, fromB] }));

    expect(() => tool.canonicalInputModel(leaf)).toThrowError(
      expect.objectContaining({ code: 'duplicate-command-parent' }),
    );
  });

  it.each([
    ['u8 bounds outside the representation', compileSchema(s.u8({ max: 300 }))],
    ['f32 bounds that do not round-trip through f32', compileSchema(s.f32({ min: 0.1 }))],
  ])('rejects ill-formed numeric restrictions: %s', (_name, codec) => {
    const tool = new ExtendedToolType(
      '1.0.0',
      command('numeric', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'value',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'ill-formed-schema' }),
    );
  });

  it('rejects a zero-length fixed-list schema', () => {
    const codec: FluentCodec = {
      graph: { defs: new Map(), root: t.fixedList(t.string(), 0) },
      toValue: () => v.fixedList([]),
      fromValue: () => [],
    };
    const tool = new ExtendedToolType(
      '1.0.0',
      command('fixed', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'value',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'ill-formed-schema' }),
    );
  });

  it('uses ECMAScript syntax when validating text regexes', () => {
    const codec: FluentCodec = {
      graph: {
        defs: new Map(),
        root: schemaType({ tag: 'text', restrictions: { regex: '(?=a)' } }),
      },
      toValue: (value) => ({ tag: 'text', text: String(value) }),
      fromValue: (value) => (value as { tag: 'text'; text: string }).text,
    };
    const tool = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'value',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).not.toThrow();
  });

  it('rejects a default whose enum case index is not representable as u32', () => {
    const codec: FluentCodec = {
      graph: { defs: new Map(), root: t.enum(['only']) },
      toValue: () => ({ tag: 'enum', caseIndex: 0.5 }),
      fromValue: () => 'only',
    };
    const tool = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'mode',
                doc: emptyDoc(),
                codec,
                default: codecValue(codec, 'only'),
                required: false,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'default-type-mismatch' }),
    );
  });

  it('rejects a default whose u64 codec emits a number instead of a bigint', () => {
    const codec: FluentCodec = {
      graph: { defs: new Map(), root: t.u64() },
      toValue: () => ({ tag: 'u64', value: 1 as unknown as bigint }),
      fromValue: () => 1n,
    };
    const tool = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'count',
                doc: emptyDoc(),
                codec,
                default: codecValue(codec, 1n),
                required: false,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'default-type-mismatch' }),
    );
  });

  it.each([
    ['bool', t.bool(), { tag: 'bool', value: 'true' }],
    ['f32', t.f32(), { tag: 'f32', value: '1.5' }],
    ['f64', t.f64(), { tag: 'f64', value: '1.5' }],
    ['string', t.string(), { tag: 'string', value: 42 }],
  ] as const)(
    'rejects a default whose %s codec emits the wrong runtime type',
    (_name, root, emitted) => {
      const codec: FluentCodec = {
        graph: { defs: new Map(), root },
        toValue: () => emitted as unknown as ReturnType<FluentCodec['toValue']>,
        fromValue: () => undefined,
      };
      const tool = new ExtendedToolType(
        '1.0.0',
        command('invalid', {
          body: body({
            positionals: {
              fixed: [
                {
                  name: 'value',
                  doc: emptyDoc(),
                  codec,
                  default: codecValue(codec, undefined),
                  required: false,
                  acceptsStdio: false,
                },
              ],
            },
          }),
        }),
      );

      expect(() => validateExtendedTool(tool)).toThrowError(
        expect.objectContaining({ code: 'default-type-mismatch' }),
      );
    },
  );

  it('rejects a union value that violates its selected discriminator', () => {
    const root = schemaType({
      tag: 'union',
      branches: [
        {
          tag: 'https',
          body: t.string(),
          discriminator: { tag: 'prefix', val: 'https://' },
          metadata: { aliases: [], examples: [] },
        },
      ],
    });

    expect(
      schemaValueConforms({ defs: new Map(), root }, root, {
        tag: 'union',
        unionTag: 'https',
        body: v.string('ftp://example.com'),
      }),
    ).toBe(false);
  });

  it('rejects tail occurrence bounds that cannot be represented as u32', () => {
    const tool = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          positionals: {
            fixed: [],
            tail: {
              name: 'items',
              doc: emptyDoc(),
              itemCodec: stringCodec,
              min: -1,
              verbatim: false,
              acceptsStdio: false,
            },
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'invalid-tail-occurrence-bounds' }),
    );
  });

  it('rejects a repeatable-option delimiter that is not one Unicode scalar', () => {
    const repeated: ExtendedOptionSpec = {
      ...option('item'),
      shape: {
        tag: 'repeatable-list',
        repetition: { tag: 'delimited', val: '::' },
        itemCodec: stringCodec,
      },
    };
    const tool = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({ options: [repeated] }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrow();
  });

  it('rejects other tool metadata values that cannot be represented by WIT', () => {
    const invalidCount = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          flags: [
            {
              ...flag('verbose'),
              shape: { tag: 'count-flag', val: 2 ** 32 },
            },
          ],
        }),
      }),
    );
    const invalidExitCode = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          errors: [
            {
              name: 'failed',
              doc: emptyDoc(),
              kind: 'runtime-error',
              exitCode: 256,
            },
          ],
        }),
      }),
    );
    const invalidShort = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({ flags: [{ ...flag('verbose'), short: '\ud800' }] }),
      }),
    );

    for (const tool of [invalidCount, invalidExitCode, invalidShort]) {
      expect(() => validateExtendedTool(tool)).toThrow();
    }
  });

  it('rejects invalid runtime-only metadata discriminants', () => {
    const tool = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          constraints: [
            {
              tag: 'implies',
              lhsQuant: 'neither' as 'all',
              lhs: [],
              rhsQuant: 'all',
              rhs: [],
            },
          ],
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'invalid-metadata-value' }),
    );
  });

  it('accepts a union input when none of its branches reaches a variant', () => {
    const codec: FluentCodec = {
      graph: {
        defs: new Map(),
        root: {
          body: {
            tag: 'union',
            branches: [
              {
                tag: 'ssh',
                body: t.string(),
                discriminator: { tag: 'prefix', val: 'ssh://' },
                metadata: { aliases: [], examples: [] },
              },
              {
                tag: 'https',
                body: t.string(),
                discriminator: { tag: 'prefix', val: 'https://' },
                metadata: { aliases: [], examples: [] },
              },
            ],
          },
          metadata: { aliases: [], examples: [] },
        },
      },
      toValue: () => {
        throw new Error('not needed by this validation test');
      },
      fromValue: () => {
        throw new Error('not needed by this validation test');
      },
    };
    const tool = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'address',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).not.toThrow();
  });

  it('rejects canonical input objects missing a declared field', () => {
    const leaf = command('show', {
      body: body({
        positionals: {
          fixed: [
            {
              name: 'source',
              doc: emptyDoc(),
              codec: stringCodec,
              required: true,
              acceptsStdio: false,
            },
          ],
        },
      }),
    });
    const input = new ExtendedToolType(
      '1.0.0',
      command('root', { subcommands: [leaf] }),
    ).canonicalInputModel(leaf);

    expect(() => input.encode({})).toThrow(/missing canonical tool input field `source`/);
  });

  it.each([
    ['duplicate record fields', t.record([field('value', t.string()), field('value', t.bool())])],
    ['an empty variant', t.variant([])],
    ['an empty enum', t.enum([])],
    ['duplicate flags', t.flags(['verbose', 'verbose'])],
    [
      'an empty union',
      {
        body: { tag: 'union' as const, branches: [] },
        metadata: { aliases: [], examples: [] },
      },
    ],
  ])('rejects an ill-formed result schema with %s', (_description, root) => {
    const codec: FluentCodec = {
      graph: { defs: new Map(), root },
      toValue: () => {
        throw new Error('not needed by this validation test');
      },
      fromValue: () => {
        throw new Error('not needed by this validation test');
      },
    };
    const tool = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          result: {
            codec,
            doc: emptyDoc(),
            formatters: [{ name: 'json', doc: emptyDoc() }],
            defaultFormatter: 'json',
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'ill-formed-schema' }),
    );
  });

  it.each([
    [
      'a quantity mantissa outside s64',
      compileSchema(
        Quantity({
          baseUnit: 'B',
          min: { mantissa: 2n ** 63n, scale: 0, unit: 'B' },
        }),
      ),
    ],
    [
      'a quantity scale outside s32',
      compileSchema(
        Quantity({
          baseUnit: 'B',
          max: { mantissa: 1n, scale: 2 ** 31, unit: 'B' },
        }),
      ),
    ],
    ['a fractional binary byte bound', compileSchema(Bytes({ minBytes: 1.5 }))],
    ['a binary byte bound outside u32', compileSchema(Bytes({ maxBytes: 2 ** 32 }))],
  ])('rejects schema restrictions not representable on the WIT wire: %s', (_description, codec) => {
    const tool = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'value',
                doc: emptyDoc(),
                codec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'ill-formed-schema' }),
    );
  });

  it('rejects a resolved value-is literal whose source and encoded values disagree', () => {
    const tool = new ExtendedToolType(
      '1.0.0',
      command('invalid', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'mode',
                doc: emptyDoc(),
                codec: stringCodec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
          constraints: [
            {
              tag: 'requires-all',
              refs: [
                {
                  tag: 'value-is',
                  name: 'mode',
                  value: {
                    tag: 'resolved',
                    codec: stringCodec,
                    value: 'prod',
                    schemaValue: v.string('dev'),
                  },
                },
              ],
            },
          ],
        }),
      }),
    );

    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'value-is-type-mismatch' }),
    );
  });
});
