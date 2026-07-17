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

import type { FlagShape, FlagSpec, OptionSpec, Tool } from 'golem:tool/common@0.1.0';
import { describe, expect, it } from 'vitest';
import { z } from 'zod/v4';
import { compileSchema } from '../src/fluent/schema/adapter';
import type { FluentCodec } from '../src/fluent/schema/codec';
import { KeyValue, Path, s } from '../src/fluent/schema/markers';
import {
  type ExtendedCommandBody,
  type ExtendedCommandNode,
  type ExtendedOptionSpec,
  ExtendedToolType,
  codecValue,
  emptyDoc,
  emptyGlobals,
  encodeTool,
} from '../src/internal/tool';

const stringCodec = compileSchema(z.string());
const pathCodec = compileSchema(Path());

function doc(summary = '') {
  return emptyDoc(summary, summary ? `${summary}.` : '');
}

function option(
  long: string,
  codec: FluentCodec = stringCodec,
  overrides: Partial<ExtendedOptionSpec> = {},
): ExtendedOptionSpec {
  return {
    long,
    aliases: [],
    doc: doc(long),
    shape: { tag: 'scalar', codec },
    required: true,
    ...overrides,
  };
}

function flag(
  long: string,
  shape: FlagShape = { tag: 'bool-flag', val: { default_: false, negatable: false } },
  overrides: Partial<FlagSpec> = {},
): FlagSpec {
  return {
    long,
    aliases: [],
    doc: doc(long),
    shape,
    ...overrides,
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
    doc: doc(name),
    globals: emptyGlobals(),
    subcommands: [],
    ...overrides,
  };
}

function typeTag(tool: Tool, index: number): string | undefined {
  return tool.schema.typeNodes[index]?.body.tag;
}

function optionTypeTag(tool: Tool, option: OptionSpec): string | undefined {
  switch (option.shape.tag) {
    case 'scalar':
    case 'optional-scalar':
      return typeTag(tool, option.shape.val);
    case 'repeatable-list':
      return typeTag(tool, option.shape.val.itemType);
    case 'repeatable-map':
      return typeTag(tool, option.shape.val.mapType);
  }
}

function grepFixture(): ExtendedToolType {
  const colorCodec = compileSchema(z.enum(['always', 'never', 'auto']));
  const maxCountCodec = compileSchema(s.u32({ min: 1 }));
  const hitsCodec = compileSchema(
    z.array(
      z.object({
        file: z.string(),
        line: z.number().int(),
        text: z.string(),
      }),
    ),
  );
  const errorCodec = compileSchema(z.object({ reason: z.string() }));
  const errors = [
    {
      name: 'invalid-pattern',
      doc: doc('Invalid pattern'),
      kind: 'usage-error' as const,
      exitCode: 2,
      payloadCodec: errorCodec,
    },
    {
      name: 'no-match',
      doc: doc('No match'),
      kind: 'runtime-error' as const,
      exitCode: 1,
    },
  ];
  const files = {
    name: 'files',
    doc: doc('Files'),
    itemCodec: pathCodec,
    min: 0,
    verbatim: false,
    acceptsStdio: true,
  };
  const replace = command('replace', {
    body: body({
      positionals: {
        fixed: [
          {
            name: 'pattern',
            doc: doc('Pattern'),
            codec: stringCodec,
            required: true,
            acceptsStdio: false,
          },
          {
            name: 'replacement',
            doc: doc('Replacement'),
            codec: stringCodec,
            required: true,
            acceptsStdio: false,
          },
        ],
        tail: files,
      },
      result: {
        codec: compileSchema(s.u64()),
        doc: doc('Replacement count'),
        formatters: [{ name: 'human', doc: doc('Human') }],
        defaultFormatter: 'human',
      },
      errors,
    }),
  });

  return new ExtendedToolType(
    '2.0.0',
    command('grep', {
      globals: {
        options: [
          option('color', colorCodec, {
            default: codecValue(colorCodec, 'auto'),
          }),
        ],
        flags: [flag('case-sensitive', undefined, { short: 'i' })],
      },
      body: body({
        positionals: {
          fixed: [
            {
              name: 'pattern',
              doc: doc('Pattern'),
              codec: stringCodec,
              required: true,
              acceptsStdio: false,
            },
          ],
          tail: files,
        },
        options: [
          option('extra-patterns', stringCodec, {
            short: 'e',
            shape: {
              tag: 'repeatable-list',
              repetition: { tag: 'either', val: ',' },
              itemCodec: stringCodec,
            },
            required: false,
          }),
          option('max-count', maxCountCodec, { short: 'n', required: false }),
        ],
        stdin: { doc: doc('Standard input'), mime: ['text/plain'], required: false },
        stdout: { doc: doc('Standard output'), mime: ['text/plain'], required: true },
        result: {
          codec: hitsCodec,
          doc: doc('Hits'),
          formatters: [
            { name: 'human', doc: doc('Human') },
            { name: 'json', doc: doc('JSON') },
          ],
          defaultFormatter: 'human',
        },
        errors,
      }),
      subcommands: [replace],
    }),
  );
}

function gitFixture(): ExtendedToolType {
  const outputCodec = compileSchema(z.enum(['human', 'porcelain', 'json']));
  const configCodec = compileSchema(KeyValue(z.string()));
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const CommitResult: z.ZodType<any> = z.object({
    hash: z.string(),
    'files-changed': z.number().int(),
    parent: z.lazy(() => CommitResult).optional(),
  });
  const commitResultCodec = compileSchema(CommitResult);
  const remoteError = {
    name: 'no-such-remote',
    doc: doc('No such remote'),
    kind: 'usage-error' as const,
    exitCode: 128,
  };
  const remoteGlobals = {
    options: [
      option('git-dir', pathCodec, {
        envVar: 'GIT_DIR',
        default: codecValue(pathCodec, '.git'),
      }),
      option('config', configCodec, {
        short: 'c',
        shape: {
          tag: 'repeatable-map' as const,
          repetition: { tag: 'repeated' as const },
          mapCodec: configCodec,
          valueCodec: stringCodec,
          duplicateKeyPolicy: 'reject' as const,
        },
        required: false,
      }),
    ],
    flags: [
      flag('verbose', { tag: 'count-flag', val: 3 }, { short: 'v' }),
      flag('paginate', { tag: 'bool-flag', val: { default_: true, negatable: true } }),
    ],
  };
  const setUrl = command('set-url', {
    body: body({
      positionals: {
        fixed: [
          {
            name: 'name',
            doc: doc('Name'),
            codec: stringCodec,
            required: true,
            acceptsStdio: false,
          },
          {
            name: 'newurl',
            doc: doc('New URL'),
            codec: compileSchema(s.url()),
            required: true,
            acceptsStdio: false,
          },
        ],
      },
      flags: [flag('add'), flag('delete')],
      constraints: [
        {
          tag: 'mutex-groups',
          groups: [[{ tag: 'present', name: 'add' }], [{ tag: 'present', name: 'delete' }]],
        },
      ],
      errors: [remoteError],
    }),
  });
  const remote = command('remote', {
    aliases: ['rmt'],
    globals: remoteGlobals,
    subcommands: [
      command('add', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'name',
                doc: doc('Name'),
                codec: stringCodec,
                required: true,
                acceptsStdio: false,
              },
              {
                name: 'url',
                doc: doc('URL'),
                codec: compileSchema(s.url()),
                required: true,
                acceptsStdio: false,
              },
            ],
          },
          options: [
            option('track', stringCodec, {
              short: 't',
              shape: {
                tag: 'repeatable-list',
                repetition: { tag: 'repeated' },
                itemCodec: stringCodec,
              },
              required: false,
            }),
          ],
          flags: [
            flag('tags', { tag: 'bool-flag', val: { default_: true, negatable: true } }),
            flag('fetch', undefined, { short: 'f' }),
          ],
          errors: [remoteError],
        }),
      }),
      command('remove', {
        aliases: ['rm'],
        body: body({
          positionals: {
            fixed: [
              {
                name: 'name',
                doc: doc('Name'),
                codec: stringCodec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
          errors: [remoteError],
        }),
      }),
      setUrl,
    ],
  });
  const commit = command('commit', {
    aliases: ['ci'],
    body: body({
      options: [
        option('message', stringCodec, { short: 'm', aliases: ['msg'] }),
        option('output', outputCodec, {
          default: codecValue(outputCodec, 'human'),
        }),
      ],
      flags: [
        flag('amend', { tag: 'bool-flag', val: { default_: false, negatable: true } }),
        flag('reset-author'),
      ],
      constraints: [
        {
          tag: 'implies',
          lhsQuant: 'all',
          lhs: [{ tag: 'present', name: 'reset-author' }],
          rhsQuant: 'all',
          rhs: [{ tag: 'present', name: 'amend' }],
        },
        {
          tag: 'requires-all',
          refs: [
            {
              tag: 'value-is',
              name: 'output',
              value: { tag: 'deferred', value: 'json' },
            },
          ],
        },
      ],
      result: {
        codec: commitResultCodec,
        doc: doc('Commit result'),
        formatters: [
          { name: 'human', doc: doc('Human') },
          { name: 'porcelain', doc: doc('Porcelain') },
          { name: 'json', doc: doc('JSON') },
        ],
        defaultFormatter: 'human',
      },
      errors: [
        {
          name: 'nothing-staged',
          doc: doc('Nothing staged'),
          kind: 'runtime-error',
          exitCode: 1,
        },
      ],
      annotations: {
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: false,
      },
    }),
  });
  const log = command('log', {
    body: body({
      positionals: {
        fixed: [],
        tail: {
          name: 'paths',
          doc: doc('Paths'),
          itemCodec: pathCodec,
          min: 0,
          separator: '--',
          verbatim: false,
          acceptsStdio: false,
        },
      },
      options: [
        option('author', stringCodec, {
          shape: {
            tag: 'repeatable-list',
            repetition: { tag: 'delimited', val: ',' },
            itemCodec: stringCodec,
          },
          required: false,
        }),
        option('grep', stringCodec, {
          shape: {
            tag: 'repeatable-list',
            repetition: { tag: 'either', val: ',' },
            itemCodec: stringCodec,
          },
          required: false,
        }),
      ],
      flags: [flag('all-match'), flag('oneline'), flag('graph')],
      constraints: [
        {
          tag: 'all-or-none',
          refs: [
            { tag: 'present', name: 'all-match' },
            { tag: 'present', name: 'grep' },
          ],
        },
        {
          tag: 'requires-any',
          refs: [
            { tag: 'present', name: 'author' },
            { tag: 'present', name: 'grep' },
          ],
        },
        {
          tag: 'forbids',
          lhsQuant: 'all',
          lhs: [{ tag: 'present', name: 'oneline' }],
          rhs: [{ tag: 'present', name: 'graph' }],
        },
      ],
      result: {
        codec: compileSchema(z.array(z.string())),
        doc: doc('Log entries'),
        formatters: [{ name: 'medium', doc: doc('Medium') }],
        defaultFormatter: 'medium',
      },
      annotations: {
        readOnly: true,
        destructive: false,
        idempotent: true,
        openWorld: false,
      },
    }),
  });

  return new ExtendedToolType(
    '0.0.0',
    command('git', {
      subcommands: [commit, remote, log],
    }),
  );
}

describe('extended tool WIT encoding', () => {
  it('encodes the canonical grep model deterministically with compiled codecs', () => {
    const fixture = grepFixture();
    const encoded = encodeTool(fixture);

    expect(encodeTool(fixture)).toEqual(encoded);
    expect(encoded.version).toBe('2.0.0');
    expect(encoded.commands.nodes.map((node) => node.name)).toEqual(['grep', 'replace']);
    expect(encoded.commands.nodes[0].subcommands).toEqual([1]);

    const root = encoded.commands.nodes[0];
    const color = root.globals.options[0];
    expect(optionTypeTag(encoded, color)).toBe('enum-type');
    expect(color.default_).toEqual({
      valueNodes: [{ tag: 'enum-value', val: 2 }],
      root: 0,
    });
    expect(root.globals.flags).toEqual([
      expect.objectContaining({ long: 'case-sensitive', short: 'i' }),
    ]);

    const commandBody = root.body!;
    expect(commandBody.positionals.fixed.map((positional) => positional.name)).toEqual(['pattern']);
    expect(typeTag(encoded, commandBody.positionals.fixed[0].type)).toBe('string-type');
    expect(commandBody.positionals.tail).toMatchObject({
      name: 'files',
      min: 0,
      acceptsStdio: true,
    });
    expect(typeTag(encoded, commandBody.positionals.tail!.itemType)).toBe('path-type');
    expect(commandBody.options.map((entry) => [entry.long, entry.shape.tag])).toEqual([
      ['extra-patterns', 'repeatable-list'],
      ['max-count', 'scalar'],
    ]);
    expect(optionTypeTag(encoded, commandBody.options[0])).toBe('string-type');
    expect(optionTypeTag(encoded, commandBody.options[1])).toBe('u32-type');
    expect(commandBody.stdin?.mime).toEqual(['text/plain']);
    expect(commandBody.stdout?.required).toBe(true);
    expect(typeTag(encoded, commandBody.result!.type)).toBe('list-type');
    expect(commandBody.result?.formatters.map((formatter) => formatter.name)).toEqual([
      'human',
      'json',
    ]);
    expect(
      commandBody.errors.map((errorCase) => [errorCase.name, errorCase.kind, errorCase.exitCode]),
    ).toEqual([
      ['invalid-pattern', 'usage-error', 2],
      ['no-match', 'runtime-error', 1],
    ]);
    expect(typeTag(encoded, commandBody.errors[0].payload!)).toBe('record-type');
    expect(encoded.schema.defs).toEqual([]);
    expect(encoded.schema.typeNodes[encoded.schema.root].body).toEqual({
      tag: 'record-type',
      val: [],
    });
  });

  it('encodes the canonical git tree, shared graph, defaults, and constraints', () => {
    const fixture = gitFixture();
    const encoded = encodeTool(fixture);

    expect(encoded.commands.nodes.map((node) => node.name)).toEqual([
      'git',
      'commit',
      'remote',
      'add',
      'remove',
      'set-url',
      'log',
    ]);
    expect(encoded.commands.nodes[0].subcommands).toEqual([1, 2, 6]);
    expect(encoded.commands.nodes[2].subcommands).toEqual([3, 4, 5]);
    expect(encoded.commands.nodes[2].body).toBeUndefined();

    const remote = encoded.commands.nodes[2];
    const gitDir = remote.globals.options.find((entry) => entry.long === 'git-dir')!;
    const config = remote.globals.options.find((entry) => entry.long === 'config')!;
    expect(optionTypeTag(encoded, gitDir)).toBe('path-type');
    expect(gitDir.default_).toEqual({
      valueNodes: [{ tag: 'path-value', val: '.git' }],
      root: 0,
    });
    expect(config.shape).toMatchObject({
      tag: 'repeatable-map',
      val: { repetition: { tag: 'repeated' }, duplicateKeyPolicy: 'reject' },
    });
    expect(optionTypeTag(encoded, config)).toBe('map-type');
    expect(remote.globals.flags.map((entry) => entry.shape)).toEqual([
      { tag: 'count-flag', val: 3 },
      { tag: 'bool-flag', val: { default_: true, negatable: true } },
    ]);

    const commit = encoded.commands.nodes[1].body!;
    const output = commit.options.find((entry) => entry.long === 'output')!;
    expect(output.default_).toEqual({
      valueNodes: [{ tag: 'enum-value', val: 0 }],
      root: 0,
    });
    expect(commit.constraints.map((constraint) => constraint.tag)).toEqual([
      'implies',
      'requires-all',
    ]);
    expect(commit.constraints[1]).toMatchObject({
      tag: 'requires-all',
      val: [
        {
          tag: 'value-is',
          val: {
            name: 'output',
            value: { valueNodes: [{ tag: 'enum-value', val: 2 }], root: 0 },
          },
        },
      ],
    });
    expect(typeTag(encoded, commit.result!.type)).toBe('ref-type');
    expect(commit.result?.defaultFormatter).toBe('human');

    const setUrl = encoded.commands.nodes[5].body!;
    expect(setUrl.constraints[0].tag).toBe('mutex-groups');
    const log = encoded.commands.nodes[6].body!;
    expect(log.constraints.map((constraint) => constraint.tag)).toEqual([
      'all-or-none',
      'requires-any',
      'forbids',
    ]);
    expect(log.options.map((entry) => entry.shape)).toMatchObject([
      { tag: 'repeatable-list', val: { repetition: { tag: 'delimited', val: ',' } } },
      { tag: 'repeatable-list', val: { repetition: { tag: 'either', val: ',' } } },
    ]);
    expect(log.positionals.tail).toMatchObject({ name: 'paths', min: 0, separator: '--' });
    expect(log.annotations).toEqual({
      readOnly: true,
      destructive: false,
      idempotent: true,
      openWorld: false,
    });
    expect(encoded.schema.defs).toHaveLength(1);
    expect(encoded.schema.defs[0].id).toMatch(/^rec:\d+$/);

    const sourceConstraint = fixture.commandByPath(['commit'])?.body?.constraints[1];
    expect(sourceConstraint).toMatchObject({
      refs: [{ tag: 'value-is', value: { tag: 'deferred', value: 'json' } }],
    });
  });

  it('encodes the parsed output of a transformed default', () => {
    const codec = compileSchema(z.string().transform((value) => `${value}!`));
    const fixture = new ExtendedToolType(
      '1.0.0',
      command('defaults', {
        body: body({
          options: [
            option('name', codec, {
              default: codecValue(codec, 'golem'),
            }),
          ],
        }),
      }),
    );

    expect(encodeTool(fixture).commands.nodes[0].body?.options[0].default_).toEqual({
      valueNodes: [{ tag: 'string-value', val: 'golem!' }],
      root: 0,
    });
  });

  it('encodes parsed item outputs in repeatable-list defaults', () => {
    const itemSchema = z.string().transform((value) => `${value}!`);
    const itemCodec = compileSchema(itemSchema);
    const defaultCodec = compileSchema(z.array(itemSchema));
    const fixture = new ExtendedToolType(
      '1.0.0',
      command('defaults', {
        body: body({
          options: [
            option('tags', itemCodec, {
              shape: {
                tag: 'repeatable-list',
                repetition: { tag: 'repeated' },
                itemCodec,
              },
              default: codecValue(defaultCodec, ['golem']),
            }),
          ],
        }),
      }),
    );

    expect(encodeTool(fixture).commands.nodes[0].body?.options[0].default_).toEqual({
      valueNodes: [
        { tag: 'string-value', val: 'golem!' },
        { tag: 'list-value', val: [0] },
      ],
      root: 1,
    });
  });

  it('encodes a transformed value-is literal whose output is not valid source input', () => {
    const codec = compileSchema(
      z
        .string()
        .regex(/^source$/)
        .transform(() => 'encoded'),
    );
    const fixture = new ExtendedToolType(
      '1.0.0',
      command('constraints', {
        body: body({
          options: [option('format', codec)],
          constraints: [
            {
              tag: 'requires-all',
              refs: [
                {
                  tag: 'value-is',
                  name: 'format',
                  value: { tag: 'deferred', value: 'source' },
                },
              ],
            },
          ],
        }),
      }),
    );

    expect(encodeTool(fixture).commands.nodes[0].body?.constraints[0]).toMatchObject({
      tag: 'requires-all',
      val: [
        {
          tag: 'value-is',
          val: {
            name: 'format',
            value: {
              valueNodes: [{ tag: 'string-value', val: 'encoded' }],
              root: 0,
            },
          },
        },
      ],
    });
  });

  it.each([
    ['secret', compileSchema(s.secret(z.string()))],
    ['quota-token', compileSchema(s.quotaToken())],
  ])('encodes a value-is snapshot containing a %s capability', (_kind, codec) => {
    const fixture = new ExtendedToolType(
      '1.0.0',
      command('constraints', {
        body: body({
          options: [option('credential', codec)],
          constraints: [
            {
              tag: 'requires-all',
              refs: [
                {
                  tag: 'value-is',
                  name: 'credential',
                  value: { tag: 'deferred', value: {} },
                },
              ],
            },
          ],
        }),
      }),
    );

    expect(() => encodeTool(fixture)).not.toThrow();
  });

  it('uses immutable codecs produced by the schema compiler', () => {
    const codec = compileSchema(z.enum(['human', 'json']));
    expect(Object.isFrozen(codec)).toBe(true);
    expect(Object.isFrozen(codec.graph)).toBe(true);
    expect(Object.isFrozen(codec.graph.root)).toBe(true);
    if (codec.graph.root.body.tag !== 'enum') throw new Error('expected enum');
    expect(Object.isFrozen(codec.graph.root.body.cases)).toBe(true);
    const definitions = codec.graph.defs as unknown as Map<string, unknown>;
    expect(() => definitions.set('other', { body: codec.graph.root })).toThrow(TypeError);
  });

  it('rejects a short form colliding with another argument long name', () => {
    const fixture = new ExtendedToolType(
      '1.0.0',
      command('collision', {
        body: body({
          options: [option('x')],
          flags: [flag('verbose', undefined, { short: 'x' })],
        }),
      }),
    );

    expect(() => encodeTool(fixture)).toThrow();
  });

  it('rejects a long name colliding with another argument short form', () => {
    const fixture = new ExtendedToolType(
      '1.0.0',
      command('collision', {
        body: body({
          options: [option('verbose', stringCodec, { short: 'x' }), option('x')],
        }),
      }),
    );

    expect(() => encodeTool(fixture)).toThrow();
  });
});
