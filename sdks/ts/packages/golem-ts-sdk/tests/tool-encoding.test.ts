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
import { c, getExtendedToolDefinition, toolDefinition } from '../src/fluent/tool';
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

function listRecordFieldTypeTag(
  tool: Tool,
  listTypeIndex: number,
  fieldName: string,
): string | undefined {
  const list = tool.schema.typeNodes[listTypeIndex]?.body;
  if (list?.tag !== 'list-type') return list?.tag;
  const record = tool.schema.typeNodes[list.val]?.body;
  if (record?.tag !== 'record-type') return record?.tag;
  const field = record.val.find((entry) => entry.name === fieldName);
  return field ? typeTag(tool, field.body) : undefined;
}

function grepFixture(): ExtendedToolType {
  const colorCodec = compileSchema(z.enum(['always', 'never', 'auto']));
  const patternCodec = compileSchema(z.string().regex(/^.+$/));
  const maxCountCodec = compileSchema(s.u32({ min: 1 }));
  const hitsCodec = compileSchema(
    z.array(
      z.object({
        file: Path(),
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
  const files = (acceptsStdio: boolean) => ({
    name: 'files',
    doc: doc('Files'),
    itemCodec: pathCodec,
    min: 0,
    verbatim: false,
    acceptsStdio,
  });
  const replace = command('replace', {
    body: body({
      positionals: {
        fixed: [
          {
            name: 'pattern',
            doc: doc('Pattern'),
            codec: patternCodec,
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
        tail: files(false),
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
              codec: patternCodec,
              required: true,
              acceptsStdio: false,
            },
          ],
          tail: files(true),
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
  const urlCodec = compileSchema(s.url());
  const datetimeCodec = compileSchema(s.datetime());
  const indexCodec = compileSchema(s.u32());
  const maxCountCodec = compileSchema(s.s64({ min: 0, max: 9223372036854775807n }));
  const commitResultCodec = compileSchema(
    z.object({
      hash: z.string(),
      'files-changed': z.number().int(),
      insertions: z.number().int(),
      deletions: z.number().int(),
    }),
  );
  const logResultCodec = compileSchema(
    z.array(
      z.object({
        hash: z.string(),
        author: z.string(),
        date: s.datetime(),
        message: z.string(),
      }),
    ),
  );
  const authorCodec = compileSchema(z.string().regex(/^.+ <.+@.+>$/));
  const remoteNameCodec = compileSchema(z.string().regex(/^[a-zA-Z][a-zA-Z0-9_-]*$/));
  const authorErrorCodec = compileSchema(z.object({ author: z.string() }));
  const stashErrorCodec = compileSchema(z.object({ name: z.string() }));
  const commitErrors = [
    {
      name: 'nothing-staged',
      doc: doc('Nothing staged'),
      kind: 'runtime-error' as const,
      exitCode: 1,
    },
    {
      name: 'dirty-merge',
      doc: doc('Dirty merge'),
      kind: 'runtime-error' as const,
      exitCode: 128,
    },
    {
      name: 'bad-author-format',
      doc: doc('Bad author format'),
      kind: 'usage-error' as const,
      exitCode: 129,
      payloadCodec: authorErrorCodec,
    },
  ];
  const logErrors = [
    {
      name: 'bad-revision',
      doc: doc('Bad revision'),
      kind: 'usage-error' as const,
      exitCode: 128,
    },
    {
      name: 'not-a-repository',
      doc: doc('Not a repository'),
      kind: 'usage-error' as const,
      exitCode: 129,
    },
  ];
  const stashError = {
    name: 'no-such-stash',
    doc: doc('No such stash'),
    kind: 'usage-error' as const,
    exitCode: 128,
    payloadCodec: stashErrorCodec,
  };
  const setUrlError = {
    name: 'failed',
    doc: doc('Failed'),
    kind: 'runtime-error' as const,
    exitCode: 1,
    payloadCodec: stringCodec,
  };
  const remoteError = {
    name: 'no-such-remote',
    doc: doc('No such remote'),
    kind: 'usage-error' as const,
    exitCode: 128,
    payloadCodec: compileSchema(z.object({ name: z.string() })),
  };
  const commonGlobals = () => ({
    options: [
      option('git-dir', pathCodec, {
        envVar: 'GIT_DIR',
        default: codecValue(pathCodec, '.git'),
        required: false,
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
      flag('verbose', { tag: 'count-flag' as const, val: 3 }, { short: 'v' }),
      flag('paginate', {
        tag: 'bool-flag' as const,
        val: { default_: true, negatable: true },
      }),
    ],
  });
  const stashGlobals = {
    options: [
      option('git-dir', pathCodec, {
        envVar: 'GIT_DIR',
        default: codecValue(pathCodec, '.git'),
        required: false,
      }),
    ],
    flags: [flag('verbose', { tag: 'count-flag', val: 3 }, { short: 'v' })],
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
            codec: urlCodec,
            required: true,
            acceptsStdio: false,
          },
          {
            name: 'oldurl',
            doc: doc('Old URL'),
            codec: urlCodec,
            required: false,
            acceptsStdio: false,
          },
        ],
      },
      flags: [flag('push'), flag('add'), flag('delete')],
      constraints: [
        {
          tag: 'mutex-groups',
          groups: [[{ tag: 'present', name: 'add' }], [{ tag: 'present', name: 'delete' }]],
        },
      ],
      errors: [setUrlError],
      annotations: {
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: true,
      },
    }),
  });
  const remote = command('remote', {
    aliases: ['rmt'],
    globals: commonGlobals(),
    subcommands: [
      command('add', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'name',
                doc: doc('Name'),
                codec: remoteNameCodec,
                required: true,
                acceptsStdio: false,
              },
              {
                name: 'url',
                doc: doc('URL'),
                codec: urlCodec,
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
            option('master', stringCodec, { short: 'm', required: false }),
          ],
          flags: [
            flag('tags', { tag: 'bool-flag', val: { default_: true, negatable: true } }),
            flag('fetch', undefined, { short: 'f' }),
          ],
          errors: [remoteError],
          annotations: {
            readOnly: false,
            destructive: false,
            idempotent: false,
            openWorld: true,
          },
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
                codec: remoteNameCodec,
                required: true,
                acceptsStdio: false,
              },
            ],
          },
          errors: [remoteError],
          annotations: {
            readOnly: false,
            destructive: true,
            idempotent: true,
            openWorld: true,
          },
        }),
      }),
      setUrl,
    ],
  });
  const commit = command('commit', {
    aliases: ['ci'],
    globals: commonGlobals(),
    body: body({
      options: [
        option('message', stringCodec, { short: 'm', aliases: ['msg'] }),
        option('author', authorCodec, {
          envVar: 'GIT_AUTHOR_NAME',
          required: false,
        }),
        option('output', outputCodec, {
          default: codecValue(outputCodec, 'human'),
          required: false,
        }),
      ],
      flags: [
        flag('amend', { tag: 'bool-flag', val: { default_: false, negatable: true } }),
        flag('signoff', { tag: 'bool-flag', val: { default_: false, negatable: true } }),
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
      errors: commitErrors,
      annotations: {
        readOnly: false,
        destructive: true,
        idempotent: false,
        openWorld: true,
      },
    }),
  });
  const stashCommandBody = body({
    options: [option('message', stringCodec, { short: 'm' })],
    flags: [flag('keep-index', undefined, { short: 'k' })],
    errors: [stashError],
  });
  const stashChild = (name: string) =>
    command(name, {
      body: body({
        positionals: {
          fixed: [
            {
              name: 'name',
              doc: doc('Name'),
              codec: stringCodec,
              required: false,
              acceptsStdio: false,
            },
          ],
        },
        options: [option('index', indexCodec, { short: 'i', required: false })],
        errors: [stashError],
      }),
    });
  const stash = command('stash', {
    globals: stashGlobals,
    body: stashCommandBody,
    subcommands: [stashChild('pop'), stashChild('apply')],
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
        option('max-count', maxCountCodec, { short: 'n', required: false }),
        option('since', datetimeCodec, { required: false }),
        option('until', datetimeCodec, { required: false }),
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
      flags: [flag('all-match'), flag('invert-grep'), flag('oneline'), flag('graph')],
      constraints: [
        {
          tag: 'all-or-none',
          refs: [
            { tag: 'present', name: 'all-match' },
            { tag: 'present', name: 'grep' },
          ],
        },
      ],
      result: {
        codec: logResultCodec,
        doc: doc('Log entries'),
        formatters: [
          { name: 'oneline', doc: doc('Oneline') },
          { name: 'short', doc: doc('Short') },
          { name: 'medium', doc: doc('Medium') },
          { name: 'full', doc: doc('Full') },
        ],
        defaultFormatter: 'medium',
      },
      errors: logErrors,
      annotations: {
        readOnly: true,
        destructive: true,
        idempotent: true,
        openWorld: true,
      },
    }),
  });

  return new ExtendedToolType(
    '0.0.0',
    command('git', {
      subcommands: [commit, remote, stash, log],
    }),
  );
}

describe('extended tool WIT encoding', () => {
  it('builds canonical tool metadata through the fluent public surface', () => {
    const definition = toolDefinition('grep')
      .version('2.0.0')
      .doc({ summary: 'Search files', description: 'Search files for patterns.' })
      .aliases('egrep')
      .global('case-sensitive', z.boolean(), { kind: 'flag', short: 'i' })
      .global('color', z.enum(['always', 'never', 'auto']), {
        default: 'auto',
        env: 'GREP_COLOR',
      })
      .annotations({ readOnly: true, idempotent: true })
      .body((commandBody) =>
        commandBody
          .positional('pattern', z.string(), { valueName: 'PATTERN' })
          .tail('files', Path({ direction: 'input', kind: 'any' }), {
            acceptsStdio: true,
          })
          .option('extra-patterns', z.string(), {
            short: 'e',
            repeatable: 'either',
            delim: ',',
          })
          .option('max-count', s.u32({ min: 1 }), { short: 'n' })
          .flag('number', { short: 'N' })
          .constraint(
            c.implies({
              lhs: c.present('extra-patterns'),
              rhs: c.present('pattern'),
            }),
          )
          .stdin({ mime: ['*/*'], required: false })
          .stdout({ mime: ['text/plain'], required: true })
          .returns(z.array(z.object({ file: z.string(), line: z.number(), text: z.string() })), {
            formatters: ['human', 'json'],
            defaultFormatter: 'human',
          })
          .error('invalid-pattern', {
            kind: 'usage',
            exitCode: 2,
            payload: z.object({ reason: z.string() }),
          })
          .error('no-match', { kind: 'runtime', exitCode: 1 }),
      )
      .command('replace', (replace) =>
        replace
          .doc('Replace matching text')
          .body((commandBody) =>
            commandBody
              .positional('pattern', z.string())
              .positional('replacement', z.string())
              .returns(z.number().int()),
          ),
      );

    const encoded = encodeTool(getExtendedToolDefinition(definition));
    expect(encoded.version).toBe('2.0.0');
    expect(encoded.commands.nodes.map((node) => node.name)).toEqual(['grep', 'replace']);
    expect(encoded.commands.nodes[0]).toMatchObject({
      aliases: ['egrep'],
      doc: { summary: 'Search files', description: 'Search files for patterns.' },
      globals: {
        options: [
          expect.objectContaining({
            long: 'color',
            required: false,
            envVar: 'GREP_COLOR',
          }),
        ],
        flags: [expect.objectContaining({ long: 'case-sensitive', short: 'i' })],
      },
    });
    const rootBody = encoded.commands.nodes[0].body!;
    expect(rootBody.positionals.fixed[0]).toMatchObject({
      name: 'pattern',
      valueName: 'PATTERN',
      required: true,
    });
    expect(rootBody.positionals.tail).toMatchObject({
      name: 'files',
      min: 0,
      acceptsStdio: true,
    });
    expect(rootBody.options.map((option) => [option.long, option.shape.tag])).toEqual([
      ['extra-patterns', 'repeatable-list'],
      ['max-count', 'scalar'],
    ]);
    expect(rootBody.constraints[0].tag).toBe('implies');
    expect(rootBody.result?.defaultFormatter).toBe('human');
    expect(rootBody.errors.map((errorCase) => [errorCase.name, errorCase.kind])).toEqual([
      ['invalid-pattern', 'usage-error'],
      ['no-match', 'runtime-error'],
    ]);
    expect(rootBody.annotations).toEqual({
      readOnly: true,
      destructive: true,
      idempotent: true,
      openWorld: true,
    });
  });

  it('builds nested dispatchers, map/count globals, all constraints, and unit results', () => {
    const definition = toolDefinition('git')
      .command('remote', (remote) =>
        remote
          .aliases('rmt')
          .global('verbose', { kind: 'count-flag', short: 'v', max: 3 })
          .global('config', KeyValue(z.string()), {
            short: 'c',
            repeatable: 'repeated',
          })
          .command('set-url', (setUrl) =>
            setUrl.body((commandBody) =>
              commandBody
                .positional('name', z.string())
                .positional('oldurl', s.url(), { required: false })
                .option('mode', z.enum(['fetch', 'push']), { optionalScalar: true })
                .flag('add')
                .flag('delete')
                .constraint(c.requiresAll([c.present('name')]))
                .constraint(c.requiresAny([c.present('add'), c.present('delete')]))
                .constraint(c.allOrNone([c.present('oldurl'), c.present('mode')]))
                .constraint(c.mutexGroups([[c.present('add')], [c.present('delete')]]))
                .constraint(c.forbids({ lhs: c.present('add'), rhs: c.present('delete') }))
                .constraint(c.requiresAll([c.valueIs('mode', 'fetch')]))
                .returns(z.void()),
            ),
          ),
      )
      .global('git-dir', Path({ kind: 'directory' }), { default: '.git' });

    const tool = getExtendedToolDefinition(definition);
    const encoded = encodeTool(tool);
    expect(encoded.version).toBe('0.0.0');
    expect(encoded.commands.nodes.map((node) => node.name)).toEqual(['git', 'remote', 'set-url']);
    expect(encoded.commands.nodes[1].body).toBeUndefined();
    expect(encoded.commands.nodes[1].globals.flags[0].shape).toEqual({
      tag: 'count-flag',
      val: 3,
    });
    expect(encoded.commands.nodes[1].globals.options[0].shape.tag).toBe('repeatable-map');
    const body = encoded.commands.nodes[2].body!;
    expect(body.result).toBeUndefined();
    expect(body.options[0].shape.tag).toBe('optional-scalar');
    expect(body.constraints.map((constraint) => constraint.tag)).toEqual([
      'requires-all',
      'requires-any',
      'all-or-none',
      'mutex-groups',
      'forbids',
      'requires-all',
    ]);
    expect(
      tool.canonicalInputModel(tool.commandByPath(['remote', 'set-url'])!).decode(
        tool.canonicalInputModel(tool.commandByPath(['remote', 'set-url'])!).encode({
          'git-dir': '.git',
          verbose: 0,
          config: new Map(),
          name: 'origin',
          oldurl: undefined,
          mode: undefined,
          add: false,
          delete: false,
        }),
      ),
    ).toMatchObject({ oldurl: undefined, mode: undefined });
  });

  it('rejects TypeScript argument-key collisions and annotations on dispatchers', () => {
    const collision = toolDefinition('collision').body((commandBody) =>
      commandBody.option('max-1', z.string()).option('max1', z.string()),
    );
    const stdinCollision = toolDefinition('stdin-collision').body((commandBody) =>
      commandBody.option('stdin', z.string()).stdin({ required: false }),
    );
    const dispatcherAnnotations = toolDefinition('dispatcher').annotations({ readOnly: true });

    expect(() => getExtendedToolDefinition(collision)).toThrowError(
      expect.objectContaining({ code: 'duplicate-name' }),
    );
    expect(() => getExtendedToolDefinition(stdinCollision)).toThrowError(
      expect.objectContaining({ code: 'duplicate-name' }),
    );
    expect(() => getExtendedToolDefinition(dispatcherAnnotations)).toThrowError(
      expect.objectContaining({ code: 'invalid-metadata-value' }),
    );
  });

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
    expect(listRecordFieldTypeTag(encoded, commandBody.result!.type, 'file')).toBe('path-type');
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
    expect(
      fixture.canonicalInputFields(fixture.commandByPath(['replace'])!).map((field) => field.name),
    ).toEqual(['color', 'case-sensitive', 'pattern', 'replacement', 'files']);
    expect(encoded.schema.defs).toEqual([]);
    expect(encoded.schema.typeNodes[encoded.schema.root].body).toEqual({
      tag: 'record-type',
      val: [],
    });
  });

  it('encodes the complete canonical git tree, defaults, constraints, and subtrees', () => {
    const fixture = gitFixture();
    const encoded = encodeTool(fixture);

    expect(encoded.commands.nodes.map((node) => node.name)).toEqual([
      'git',
      'commit',
      'remote',
      'add',
      'remove',
      'set-url',
      'stash',
      'pop',
      'apply',
      'log',
    ]);
    expect(encoded.commands.nodes[0].subcommands).toEqual([1, 2, 6, 9]);
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
    expect(
      fixture
        .canonicalInputFields(fixture.commandByPath(['remote', 'add'])!)
        .map((field) => field.name),
    ).toEqual([
      'git-dir',
      'config',
      'verbose',
      'paginate',
      'name',
      'url',
      'track',
      'master',
      'tags',
      'fetch',
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
    expect(commit.annotations).toEqual({
      readOnly: false,
      destructive: true,
      idempotent: false,
      openWorld: true,
    });
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
    expect(typeTag(encoded, commit.result!.type)).toBe('record-type');
    expect(commit.result?.defaultFormatter).toBe('human');
    expect(commit.errors.map((errorCase) => [errorCase.name, errorCase.kind])).toEqual([
      ['nothing-staged', 'runtime-error'],
      ['dirty-merge', 'runtime-error'],
      ['bad-author-format', 'usage-error'],
    ]);

    const setUrl = encoded.commands.nodes[5].body!;
    expect(setUrl.constraints[0].tag).toBe('mutex-groups');
    expect(setUrl.positionals.fixed.map((positional) => positional.required)).toEqual([
      true,
      true,
      false,
    ]);
    expect(setUrl.flags.map((entry) => entry.long)).toEqual(['push', 'add', 'delete']);

    const stash = encoded.commands.nodes[6];
    expect(stash.body).toBeDefined();
    expect(stash.subcommands).toEqual([7, 8]);
    expect(stash.body?.options.map((entry) => entry.long)).toEqual(['message']);
    expect(stash.body?.flags.map((entry) => entry.long)).toEqual(['keep-index']);
    expect(
      fixture.canonicalInputFields(fixture.commandByPath(['stash'])!).map((field) => field.name),
    ).toEqual(['git-dir', 'verbose', 'message', 'keep-index']);
    expect(fixture.projectHelp(['stash'])).toMatchObject({
      command: { name: 'stash' },
      arguments: [
        { name: 'git-dir', kind: 'global-option' },
        { name: 'verbose', kind: 'global-flag' },
        { name: 'message', kind: 'option' },
        { name: 'keep-index', kind: 'flag' },
      ],
      subcommands: [{ name: 'pop' }, { name: 'apply' }],
    });
    expect(
      fixture
        .canonicalInputFields(fixture.commandByPath(['stash', 'pop'])!)
        .map((field) => field.name),
    ).toEqual(['git-dir', 'verbose', 'name', 'index']);

    const log = encoded.commands.nodes[9].body!;
    expect(log.constraints.map((constraint) => constraint.tag)).toEqual(['all-or-none']);
    expect(log.options.map((entry) => [entry.long, entry.shape])).toMatchObject([
      ['max-count', { tag: 'scalar' }],
      ['since', { tag: 'scalar' }],
      ['until', { tag: 'scalar' }],
      ['author', { tag: 'repeatable-list', val: { repetition: { tag: 'delimited', val: ',' } } }],
      ['grep', { tag: 'repeatable-list', val: { repetition: { tag: 'either', val: ',' } } }],
    ]);
    expect(log.positionals.tail).toMatchObject({ name: 'paths', min: 0, separator: '--' });
    expect(log.flags.map((entry) => entry.long)).toEqual([
      'all-match',
      'invert-grep',
      'oneline',
      'graph',
    ]);
    expect(log.result?.formatters.map((formatter) => formatter.name)).toEqual([
      'oneline',
      'short',
      'medium',
      'full',
    ]);
    expect(listRecordFieldTypeTag(encoded, log.result!.type, 'date')).toBe('datetime-type');
    expect(log.errors.map((errorCase) => errorCase.name)).toEqual([
      'bad-revision',
      'not-a-repository',
    ]);
    expect(log.annotations).toEqual({
      readOnly: true,
      destructive: true,
      idempotent: true,
      openWorld: true,
    });

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

  it('encodes a value-is literal accepted by a later peeled collection codec', () => {
    const codec = compileSchema(z.array(z.string()));
    const fixture = new ExtendedToolType(
      '1.0.0',
      command('constraints', {
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

    expect(encodeTool(fixture).commands.nodes[0].body?.constraints[0]).toMatchObject({
      tag: 'requires-all',
      val: [
        {
          tag: 'value-is',
          val: {
            name: 'items',
            value: {
              valueNodes: [{ tag: 'string-value', val: 'needle' }],
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

  it('allows short forms to overlap long names and aliases', () => {
    const fixture = new ExtendedToolType(
      '1.0.0',
      command('collision', {
        body: body({
          options: [option('x'), option('other', stringCodec, { aliases: ['y'] })],
          flags: [
            flag('verbose', undefined, { short: 'x' }),
            flag('quiet', undefined, { short: 'y' }),
          ],
        }),
      }),
    );

    const encoded = encodeTool(fixture);
    expect(encoded.commands.nodes[0].body).toMatchObject({
      options: [{ long: 'x' }, { long: 'other', aliases: ['y'] }],
      flags: [
        { long: 'verbose', short: 'x' },
        { long: 'quiet', short: 'y' },
      ],
    });
    expect(fixture.canonicalInputModel(fixture.root).fields.map((field) => field.name)).toEqual([
      'x',
      'other',
      'verbose',
      'quiet',
    ]);
  });

  it('allows inherited and local surfaces to overlap across namespaces in either direction', () => {
    const fixture = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: {
          options: [option('x'), option('other', stringCodec, { aliases: ['y'], short: 'z' })],
          flags: [],
        },
        subcommands: [
          command('leaf', {
            body: body({
              options: [option('z')],
              flags: [
                flag('verbose', undefined, { short: 'x' }),
                flag('quiet', undefined, { short: 'y' }),
              ],
            }),
          }),
        ],
      }),
    );

    const encoded = encodeTool(fixture);
    expect(encoded.commands.nodes[1].body).toMatchObject({
      options: [{ long: 'z' }],
      flags: [
        { long: 'verbose', short: 'x' },
        { long: 'quiet', short: 'y' },
      ],
    });
  });

  it('still rejects duplicate long surfaces and duplicate short forms within their namespaces', () => {
    const duplicateName = new ExtendedToolType(
      '1.0.0',
      command('duplicate-name', {
        body: body({
          options: [option('profile'), option('other', stringCodec, { aliases: ['profile'] })],
        }),
      }),
    );
    const duplicateShort = new ExtendedToolType(
      '1.0.0',
      command('duplicate-short', {
        body: body({
          options: [option('profile', stringCodec, { short: 'p' })],
          flags: [flag('print', undefined, { short: 'p' })],
        }),
      }),
    );

    expect(() => encodeTool(duplicateName)).toThrowError(
      expect.objectContaining({ code: 'duplicate-name' }),
    );
    expect(() => encodeTool(duplicateShort)).toThrowError(
      expect.objectContaining({ code: 'duplicate-short' }),
    );
  });
});
