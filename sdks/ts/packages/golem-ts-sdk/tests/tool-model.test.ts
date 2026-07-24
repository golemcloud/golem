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
import {
  getExtendedToolDefinition,
  renderArgumentHelp,
  renderHelp,
  toolDefinition,
} from '../src/fluent/tool';
import {
  field,
  schemaShapesMatch,
  schemaType,
  t,
  v,
  validateSchemaGraph,
} from '../src/internal/schema-model';
import {
  CanonicalInputModel,
  type ExtendedCommandBody,
  type ExtendedCommandNode,
  type ExtendedOptionSpec,
  ExtendedToolType,
  appendGraftedSubtree,
  codecValue,
  emptyDoc,
  emptyGlobals,
  graftSubtree,
  listCodec,
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
  it('compares canonical schema shape across restrictions and recursive definition names', () => {
    expect(
      schemaShapesMatch(
        {
          defs: new Map(),
          root: t.s32({ min: { tag: 'signed', val: 0n } }),
        },
        {
          defs: new Map(),
          root: t.s32({ max: { tag: 'signed', val: 10n } }),
        },
      ),
    ).toBe(true);
    expect(
      schemaShapesMatch({ defs: new Map(), root: t.s32() }, { defs: new Map(), root: t.u32() }),
    ).toBe(false);
    expect(
      schemaShapesMatch(
        { defs: new Map(), root: t.record([field('left', t.string())]) },
        { defs: new Map(), root: t.record([field('right', t.string())]) },
      ),
    ).toBe(false);

    const recursive = (id: string) => ({
      defs: new Map([
        [
          id,
          {
            body: t.record([field('next', t.option(t.ref(id)))]),
          },
        ],
      ]),
      root: t.ref(id),
    });
    expect(schemaShapesMatch(recursive('left-node'), recursive('right-node'))).toBe(true);
  });

  it('does not conflate recursive ref pairs containing the pair delimiter', () => {
    const left = {
      defs: new Map([
        [
          'a\u0000b',
          {
            body: t.record([field('next', t.option(t.ref('a\u0000b')))]),
          },
        ],
        ['a', { body: t.string() }],
      ]),
      root: t.record([field('recursive', t.ref('a\u0000b')), field('mismatch', t.ref('a'))]),
    };
    const right = {
      defs: new Map([
        [
          'c',
          {
            body: t.record([field('next', t.option(t.ref('c')))]),
          },
        ],
        ['b\u0000c', { body: t.u32() }],
      ]),
      root: t.record([field('recursive', t.ref('c')), field('mismatch', t.ref('b\u0000c'))]),
    };

    expect(validateSchemaGraph(left)).toEqual([]);
    expect(validateSchemaGraph(right)).toEqual([]);
    expect(schemaShapesMatch(left, right)).toBe(false);
  });

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
        subcommands: [
          command('remote', {
            globals: { options: [option('tenant')], flags: [flag('quiet')] },
            subcommands: [leaf],
          }),
        ],
      }),
    );

    expect(tool.commandByPath(['remote', 's'])).toBe(leaf);
    expect(tool.commandPath(leaf)).toEqual(['remote', 'search']);
    const input = tool.canonicalInputModel(leaf);
    expect(input.fields.map((field) => field.name)).toEqual([
      'profile',
      'verbose',
      'tenant',
      'quiet',
      'endpoint',
      'source',
      'patterns',
      'format',
      'dry-run',
    ]);
    const value = {
      profile: 'prod',
      verbose: true,
      tenant: 'golem',
      quiet: false,
      endpoint: 'local',
      source: 'src',
      patterns: ['TODO', 'FIXME'],
      format: 'json',
      'dry-run': false,
    };
    expect(input.decode(input.encode(value))).toEqual(value);
    expect(tool.projectHelp(['remote', 'search'])?.arguments.map((entry) => entry.kind)).toEqual([
      'global-option',
      'global-flag',
      'global-option',
      'global-flag',
      'global-option',
      'positional',
      'tail',
      'option',
      'flag',
    ]);
  });

  it('renders descriptor help at every depth with canonical paths and rich metadata', () => {
    const definition = toolDefinition('git')
      .aliases('g')
      .doc({
        summary: 'Version control',
        description: 'Manage source repositories.',
        examples: [{ title: 'Status', body: 'git status' }],
      })
      .global('profile', z.string(), {
        aliases: ['profile-name'],
        short: 'p',
        doc: { summary: 'Configuration profile', description: 'Selects a profile.' },
        valueName: 'PROFILE',
        default: 'prod',
        env: 'GIT_PROFILE',
      })
      .global('verbose', {
        kind: 'count-flag',
        aliases: ['verbosity'],
        short: 'v',
        doc: 'Increase verbosity',
        env: 'GIT_VERBOSE',
        max: 3,
      })
      .command('remote', (remote) =>
        remote
          .aliases('r')
          .doc('Manage remotes')
          .global('endpoint', z.string(), {
            doc: 'Remote endpoint',
            required: true,
          })
          .command('add', (add) =>
            add
              .aliases('a')
              .doc({
                summary: 'Add a remote',
                description: 'Registers a named remote.',
                examples: [{ title: 'Origin', body: 'git remote add origin URL' }],
              })
              .annotations({ readOnly: false, destructive: false, idempotent: true })
              .body((body) =>
                body
                  .positional('name', z.string(), {
                    doc: { summary: 'Remote name', description: 'Name to register.' },
                    valueName: 'NAME',
                  })
                  .tail('refspecs', z.string(), {
                    doc: 'Refspecs',
                    valueName: 'REFSPEC',
                    min: 1,
                    max: 3,
                    separator: '--',
                    verbatim: true,
                    acceptsStdio: true,
                  })
                  .option('fetch-depth', s.u32(), {
                    aliases: ['depth'],
                    short: 'd',
                    doc: {
                      summary: 'Fetch depth',
                      description: 'Limits fetched history.',
                      examples: [{ title: 'Shallow', body: '--fetch-depth 1' }],
                    },
                    valueName: 'DEPTH',
                    required: true,
                    default: 1,
                    env: 'GIT_DEPTH',
                  })
                  .flag('force', {
                    aliases: ['overwrite'],
                    short: 'f',
                    doc: 'Replace an existing remote',
                    default: true,
                    negatable: true,
                    env: 'GIT_FORCE',
                  })
                  .stdin({ doc: 'Refspec input', mime: ['text/plain'], required: true })
                  .stdout({ doc: 'Progress output', mime: ['text/plain'] })
                  .returns(z.object({ added: z.boolean() }), {
                    doc: 'Added remote',
                    formatters: [
                      {
                        name: 'human',
                        doc: {
                          summary: 'Human-readable result',
                          description: '',
                          examples: [],
                        },
                      },
                      {
                        name: 'json',
                        doc: { summary: 'JSON result', description: '', examples: [] },
                      },
                    ],
                    defaultFormatter: 'json',
                  })
                  .error('already-exists', {
                    kind: 'usage',
                    exitCode: 2,
                    doc: 'Remote already exists',
                    payload: z.object({ name: z.string() }),
                  }),
              ),
          ),
      );

    const root = renderHelp(definition);
    expect(root.tag).toBe('ok');
    if (root.tag !== 'ok') throw new Error('expected root help');
    expect(root.value).toContain('Usage: git');
    expect(root.value).toContain('Aliases: g');
    expect(root.value).toContain('Status:');
    expect(root.value).toContain('--profile, --profile-name, -p <PROFILE>');
    expect(root.value).toContain('[optional; default: prod; env: GIT_PROFILE]');
    expect(root.value).toContain('remote (aliases: r)');

    const dispatcher = renderHelp(definition, ['r']);
    expect(dispatcher.tag).toBe('ok');
    if (dispatcher.tag !== 'ok') throw new Error('expected dispatcher help');
    expect(dispatcher.value).toContain('Usage: git remote');
    expect(dispatcher.value).toContain('--profile, --profile-name, -p <PROFILE>');
    expect(dispatcher.value).toContain('--endpoint [required]');
    expect(dispatcher.value).toContain('add (aliases: a)');

    const nested = renderHelp(definition, ['r', 'a']);
    expect(nested.tag).toBe('ok');
    if (nested.tag !== 'ok') throw new Error('expected nested help');
    expect(nested.value).toContain('Usage: git remote add');
    expect(nested.value).toContain('Aliases: a');
    expect(nested.value).toContain('--profile, --profile-name, -p <PROFILE>');
    expect(nested.value).toContain('--endpoint [required]');
    expect(nested.value).toContain('name <NAME> [required]');
    expect(nested.value).toContain(
      'refspecs... <REFSPEC>... [required; accepts stdio; min: 1; max: 3; separator: --; verbatim]',
    );
    expect(nested.value).toContain('--fetch-depth, --depth, -d <DEPTH>');
    expect(nested.value).toContain('[required; default: 1; env: GIT_DEPTH]');
    expect(nested.value).toContain(
      '--force, --overwrite, -f [default: true; env: GIT_FORCE; negatable]',
    );
    expect(nested.value).toContain('Stdin:\n  required; MIME: text/plain');
    expect(nested.value).toContain('Stdout:\n  optional; MIME: text/plain');
    expect(nested.value).toContain('Formatters: human, json (default)');
    expect(nested.value).toContain('already-exists [usage-error; exit: 2; payload]');
    expect(nested.value).toContain('read-only: false');
    expect(nested.value).toContain('idempotent: true');

    expect(getExtendedToolDefinition(definition)).toBe(getExtendedToolDefinition(definition));
    expect(getExtendedToolDefinition(definition).projectHelp(['r', 'a'])?.commandPath).toEqual([
      'remote',
      'add',
    ]);

    const tail = renderArgumentHelp(definition, ['r', 'a'], 'refspecs');
    expect(tail.tag).toBe('ok');
    if (tail.tag !== 'ok') throw new Error('expected tail help');
    expect(tail.value).toContain('Minimum occurrences: 1');
    expect(tail.value).toContain('Maximum occurrences: 3');
    expect(tail.value).toContain('Separator: --');
    expect(tail.value).toContain('Verbatim: true');
    expect(tail.value).toContain('Accepts standard input: true');

    const booleanFlag = renderArgumentHelp(definition, ['r', 'a'], 'overwrite');
    expect(booleanFlag.tag).toBe('ok');
    if (booleanFlag.tag !== 'ok') throw new Error('expected boolean flag help');
    expect(booleanFlag.value).toContain('Default: true');
    expect(booleanFlag.value).toContain('Negatable: true');

    const countFlag = renderArgumentHelp(definition, ['r', 'a'], 'verbosity');
    expect(countFlag.tag).toBe('ok');
    if (countFlag.tag !== 'ok') throw new Error('expected count flag help');
    expect(countFlag.value).toContain('Maximum count: 3');
  });

  it('renders argument help by canonical name or alias and returns structured lookup errors', () => {
    const definition = toolDefinition('grep')
      .global('profile', z.string(), {
        aliases: ['configuration'],
        short: 'p',
        doc: {
          summary: 'Profile',
          description: 'Configuration profile.',
          examples: [{ title: 'Production', body: '--profile prod' }],
        },
        valueName: 'PROFILE',
        default: 'prod',
        env: 'GREP_PROFILE',
      })
      .command('search', (search) =>
        search
          .aliases('s')
          .body((body) =>
            body
              .positional('pattern', z.string(), { doc: 'Pattern' })
              .option('color', z.string(), { aliases: ['colour'], doc: 'Color mode' }),
          ),
      );

    const global = renderArgumentHelp(definition, ['s'], 'configuration');
    expect(global.tag).toBe('ok');
    if (global.tag !== 'ok') throw new Error('expected global argument help');
    expect(global.value).toContain('--profile, --configuration, -p <PROFILE> (option, global)');
    expect(global.value).toContain('Aliases: --configuration, -p');
    expect(global.value).toContain('Required: false');
    expect(global.value).toContain('Default: prod');
    expect(global.value).toContain('Environment: GREP_PROFILE');
    expect(global.value).toContain('Production:');

    const positional = renderArgumentHelp(definition, ['search'], 'pattern');
    expect(positional).toEqual(expect.objectContaining({ tag: 'ok' }));
    if (positional.tag !== 'ok') throw new Error('expected positional help');
    expect(positional.value).toContain('pattern (positional, required)');

    expect(renderHelp(definition, ['missing'])).toEqual({
      tag: 'err',
      error: { tag: 'invalid-command-path', commandPath: ['missing'] },
    });
    expect(renderArgumentHelp(definition, ['missing'], 'profile')).toEqual({
      tag: 'err',
      error: { tag: 'invalid-command-path', commandPath: ['missing'] },
    });
    expect(renderArgumentHelp(definition, ['s'], 'missing')).toEqual({
      tag: 'err',
      error: {
        tag: 'invalid-argument-name',
        commandPath: ['search'],
        argumentName: 'missing',
      },
    });
    expect(renderArgumentHelp(definition, ['s'], 'p')).toEqual({
      tag: 'err',
      error: {
        tag: 'invalid-argument-name',
        commandPath: ['search'],
        argumentName: 'p',
      },
    });

    const transformedDefault = toolDefinition('defaults').body((body) =>
      body.option(
        'name',
        z.string().transform((value) => `${value}!`),
        {
          default: 'golem',
        },
      ),
    );
    const transformedDefaultHelp = renderArgumentHelp(transformedDefault, [], 'name');
    expect(transformedDefaultHelp.tag).toBe('ok');
    if (transformedDefaultHelp.tag !== 'ok') throw new Error('expected transformed default help');
    expect(transformedDefaultHelp.value).toContain('Default: golem!');

    const emptySeparator = toolDefinition('tail').body((body) =>
      body.tail('args', z.string(), { separator: '' }),
    );
    const emptySeparatorHelp = renderArgumentHelp(emptySeparator, [], 'args');
    expect(emptySeparatorHelp.tag).toBe('ok');
    if (emptySeparatorHelp.tag !== 'ok') throw new Error('expected empty separator help');
    expect(emptySeparatorHelp.value).toContain('Separator: ');

    for (const value of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
      const floatDefault = toolDefinition('float').body((body) =>
        body.option('threshold', s.f32(), { default: value }),
      );
      const floatDefaultHelp = renderArgumentHelp(floatDefault, [], 'threshold');
      expect(floatDefaultHelp.tag).toBe('ok');
      if (floatDefaultHelp.tag !== 'ok') throw new Error('expected float default help');
      expect(floatDefaultHelp.value).toContain(`Default: ${String(value)}`);
    }
  });

  it('renders non-finite numbers inside repeatable defaults without replacing them with null', () => {
    const definition = toolDefinition('float-list').body((body) =>
      body.option('threshold', s.f32(), {
        repeatable: 'repeated',
        default: [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY],
      }),
    );

    const help = renderArgumentHelp(definition, [], 'threshold');
    expect(help.tag).toBe('ok');
    if (help.tag !== 'ok') throw new Error('expected float-list default help');
    expect(help.value).toContain('NaN');
    expect(help.value).toContain('Infinity');
    expect(help.value).toContain('-Infinity');
    expect(help.value).not.toContain('null');
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

  it.each([
    ['long name', option('account'), 'duplicate-name'],
    ['alias', { ...option('local'), aliases: ['profile'] }, 'duplicate-name'],
    ['short name', { ...option('local'), short: 'p' }, 'duplicate-short'],
  ] as const)('rejects a local %s colliding with an inherited global', (_surface, local, code) => {
    const inherited = { ...option('profile'), aliases: ['account'], short: 'p' };
    const tool = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: { options: [inherited], flags: [] },
        subcommands: [command('leaf', { body: body({ options: [local] }) })],
      }),
    );

    expect(() => normalizeExtendedTool(tool)).toThrowError(expect.objectContaining({ code }));
  });

  it('reconciles a grafted child root against its new ancestor globals', () => {
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        globals: { options: [option('profile')], flags: [] },
        body: body(),
      }),
    );
    const parent = command('root', {
      globals: { options: [option('profile')], flags: [] },
    });
    const composed = new ExtendedToolType(
      '1.0.0',
      appendGraftedSubtree(parent, graftSubtree(child, { expectedName: 'remote' })),
    );

    const normalized = normalizeExtendedTool(composed);
    const remote = normalized.commandByPath(['remote']);
    expect(remote?.globals.options).toEqual([]);
    expect(
      normalized
        .effectiveGlobals(remote!)
        .map((entry) => (entry.tag === 'option' ? entry.option.long : undefined)),
    ).toEqual(['profile']);
    expect(normalized.canonicalInputFields(remote!).map((field) => field.name)).toEqual([
      'profile',
    ]);
    expect(child.root.globals.options.map((entry) => entry.long)).toEqual(['profile']);
    const standalone = normalizeExtendedTool(child);
    expect(standalone.canonicalInputFields(standalone.root).map((field) => field.name)).toEqual([
      'profile',
    ]);
  });

  it('de-projects compatible graft-root body fields by long names and aliases', () => {
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        body: body({
          options: [{ ...option('config'), aliases: ['profile'], doc: emptyDoc('child config') }],
        }),
      }),
    );
    const parentOption = {
      ...option('profile'),
      aliases: ['config'],
      doc: emptyDoc('parent profile'),
    };
    const composed = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: { options: [parentOption], flags: [] },
        subcommands: [graftSubtree(child, { expectedName: 'remote' })],
      }),
    );

    const normalized = normalizeExtendedTool(composed);
    const remote = normalized.commandByPath(['remote'])!;
    expect(remote.body?.options).toEqual([]);
    expect(normalized.canonicalInputFields(remote).map((field) => field.name)).toEqual(['profile']);
    expect(normalized.projectHelp(['remote'])?.arguments).toEqual([
      expect.objectContaining({ name: 'profile', aliases: ['config'], doc: parentOption.doc }),
    ]);
    expect(child.root.body?.options.map((entry) => entry.long)).toEqual(['config']);
  });

  it('de-projects a graft-root alias matched to an inherited alias', () => {
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        body: body({ options: [{ ...option('child-profile'), aliases: ['shared'] }] }),
      }),
    );
    const composed = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: {
          options: [{ ...option('parent-profile'), aliases: ['shared'] }],
          flags: [],
        },
        subcommands: [graftSubtree(child, { expectedName: 'remote' })],
      }),
    );

    const remote = normalizeExtendedTool(composed).commandByPath(['remote'])!;
    expect(remote.body?.options).toEqual([]);
  });

  it('rejects child-only constraint aliases removed by inherited-global reconciliation', () => {
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        body: body({
          options: [{ ...option('config'), aliases: ['settings', 'shared'] }],
          constraints: [
            { tag: 'requires-all', refs: [{ tag: 'present', name: 'settings' }] },
            {
              tag: 'requires-all',
              refs: [
                {
                  tag: 'value-is',
                  name: 'settings',
                  value: { tag: 'deferred', value: 'prod' },
                },
              ],
            },
          ],
        }),
      }),
    );
    const composed = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: {
          options: [{ ...option('profile'), aliases: ['shared'] }],
          flags: [],
        },
        subcommands: [graftSubtree(child, { expectedName: 'remote' })],
      }),
    );

    expect(() => normalizeExtendedTool(composed)).toThrowError(
      expect.objectContaining({ code: 'unresolved-constraint-ref' }),
    );
  });

  it('resolves a shared constraint surface using the surviving ancestor restrictions', () => {
    const parentCodec = compileSchema(z.string().regex(/^prod$/));
    const childCodec = compileSchema(z.string().regex(/^dev$/));
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        body: body({
          options: [option('config', childCodec)],
          constraints: [
            {
              tag: 'requires-all',
              refs: [
                {
                  tag: 'value-is',
                  name: 'config',
                  value: { tag: 'deferred', value: 'prod' },
                },
              ],
            },
          ],
        }),
      }),
    );
    expect(() => normalizeExtendedTool(child)).toThrowError(
      expect.objectContaining({ code: 'value-is-type-mismatch' }),
    );

    const composed = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: {
          options: [{ ...option('profile', parentCodec), aliases: ['config'] }],
          flags: [],
        },
        subcommands: [graftSubtree(child, { expectedName: 'remote' })],
      }),
    );
    const remote = normalizeExtendedTool(composed).commandByPath(['remote'])!;

    expect(remote.body?.options).toEqual([]);
    expect(remote.body?.constraints).toMatchObject([
      {
        refs: [
          {
            tag: 'value-is',
            name: 'config',
            value: { tag: 'resolved', schemaValue: v.string('prod') },
          },
        ],
      },
    ]);
  });

  it('de-projects compatible collected values and matching flag families', () => {
    const optionalOption: ExtendedOptionSpec = {
      ...option('optional'),
      shape: { tag: 'optional-scalar', codec: stringCodec },
    };
    const listOption: ExtendedOptionSpec = {
      ...option('items'),
      shape: {
        tag: 'repeatable-list',
        repetition: { tag: 'repeated' },
        itemCodec: stringCodec,
      },
    };
    const mapCodec = compileSchema(KeyValue(z.string()));
    const mapOption: ExtendedOptionSpec = {
      ...option('labels'),
      shape: {
        tag: 'repeatable-map',
        repetition: { tag: 'repeated' },
        mapCodec,
        valueCodec: mapCodec.mapValue!,
        duplicateKeyPolicy: 'reject',
      },
    };
    const boolFlag = flag('force');
    const countFlag: FlagSpec = {
      ...flag('verbose'),
      shape: { tag: 'count-flag', val: 3 },
    };
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        body: body({
          options: [optionalOption, listOption, mapOption],
          flags: [boolFlag, countFlag],
        }),
      }),
    );
    const composed = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: {
          options: [optionalOption, listOption, mapOption],
          flags: [boolFlag, countFlag],
        },
        subcommands: [graftSubtree(child, { expectedName: 'remote' })],
      }),
    );

    const remote = normalizeExtendedTool(composed).commandByPath(['remote'])!;
    expect(remote.body?.options).toEqual([]);
    expect(remote.body?.flags).toEqual([]);
  });

  it('de-projects compatible fixed and tail positionals at a graft root', () => {
    const items: ExtendedOptionSpec = {
      ...option('items'),
      shape: {
        tag: 'repeatable-list',
        repetition: { tag: 'repeated' },
        itemCodec: stringCodec,
      },
    };
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
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
            tail: {
              name: 'items',
              doc: emptyDoc(),
              itemCodec: stringCodec,
              min: 0,
              verbatim: false,
              acceptsStdio: false,
            },
          },
        }),
      }),
    );
    const composed = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: { options: [option('profile'), items], flags: [] },
        subcommands: [graftSubtree(child, { expectedName: 'remote' })],
      }),
    );

    const remote = normalizeExtendedTool(composed).commandByPath(['remote'])!;
    expect(remote.body?.positionals.fixed).toEqual([]);
    expect(remote.body?.positionals.tail).toBeUndefined();
  });

  it('rejects incompatible and ambiguous inherited-global matches precisely', () => {
    const incompatibleChild = new ExtendedToolType(
      '1.0.0',
      command('remote', { body: body({ options: [option('force')] }) }),
    );
    const incompatible = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: { options: [], flags: [flag('force')] },
        subcommands: [graftSubtree(incompatibleChild, { expectedName: 'remote' })],
      }),
    );
    const ambiguousChild = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        body: body({ options: [{ ...option('shared'), aliases: ['second'] }] }),
      }),
    );
    const ambiguous = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: {
          options: [
            { ...option('first'), aliases: ['shared'] },
            { ...option('other'), aliases: ['second'] },
          ],
          flags: [],
        },
        subcommands: [graftSubtree(ambiguousChild, { expectedName: 'remote' })],
      }),
    );

    expect(() => normalizeExtendedTool(incompatible)).toThrowError(
      expect.objectContaining({ code: 'inherited-global-incompatible' }),
    );
    expect(() => normalizeExtendedTool(ambiguous)).toThrowError(
      expect.objectContaining({ code: 'inherited-global-ambiguous' }),
    );
  });

  it('classifies multiple inherited matches as ambiguous even when one is incompatible', () => {
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        body: body({ options: [{ ...option('shared'), aliases: ['force'] }] }),
      }),
    );
    const composed = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: {
          options: [{ ...option('profile'), aliases: ['shared'] }],
          flags: [{ ...flag('enabled'), aliases: ['force'] }],
        },
        subcommands: [graftSubtree(child, { expectedName: 'remote' })],
      }),
    );

    expect(() => normalizeExtendedTool(composed)).toThrowError(
      expect.objectContaining({ code: 'inherited-global-ambiguous' }),
    );
  });

  it('does not use a short-only overlap as inherited-global identity', () => {
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
        body: body({ options: [{ ...option('local'), short: 'p' }] }),
      }),
    );
    const composed = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: { options: [{ ...option('profile'), short: 'p' }], flags: [] },
        subcommands: [graftSubtree(child, { expectedName: 'remote' })],
      }),
    );

    expect(() => normalizeExtendedTool(composed)).toThrowError(
      expect.objectContaining({ code: 'duplicate-short' }),
    );
  });

  it('resolves a grafted child constraint against a new ancestor global', () => {
    const child = new ExtendedToolType(
      '1.0.0',
      command('remote', {
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
      }),
    );
    expect(() => normalizeExtendedTool(child)).toThrowError(
      expect.objectContaining({ code: 'unresolved-constraint-ref' }),
    );

    const parent = command('root', {
      globals: { options: [option('profile')], flags: [] },
    });
    const composed = new ExtendedToolType(
      '1.0.0',
      appendGraftedSubtree(parent, graftSubtree(child, { expectedName: 'remote' })),
    );
    const first = normalizeExtendedTool(composed);
    const second = normalizeExtendedTool(composed);
    const constraint = first.commandByPath(['remote'])?.body?.constraints[0];

    expect(constraint).toMatchObject({
      tag: 'requires-all',
      refs: [
        {
          tag: 'value-is',
          name: 'profile',
          value: {
            tag: 'resolved',
            value: 'prod',
            schemaValue: { tag: 'string', value: 'prod' },
          },
        },
      ],
    });
    expect(second).toEqual(first);
  });

  it('uses body-local fields as the canonical fallback for invalid inherited collisions', () => {
    const leaf = command('leaf', {
      body: body({ options: [{ ...option('local'), aliases: ['profile'] }] }),
    });
    const tool = new ExtendedToolType(
      '1.0.0',
      command('root', {
        globals: { options: [{ ...option('profile'), aliases: ['p'] }], flags: [] },
        subcommands: [leaf],
      }),
    );

    expect(tool.canonicalInputFields(leaf).map((field) => field.name)).toEqual(['local']);
    expect(() => validateExtendedTool(tool)).toThrowError(
      expect.objectContaining({ code: 'duplicate-name' }),
    );
  });

  it('forwards canonical values by name or alias in target field order', () => {
    const source = new CanonicalInputModel([
      { name: 'profile', aliases: ['p'], codec: stringCodec },
      { name: 'source', aliases: [], codec: stringCodec },
    ]);
    const target = new CanonicalInputModel([
      { name: 'source', aliases: [], codec: stringCodec },
      { name: 'p', aliases: [], codec: stringCodec },
    ]);
    const sourceInput = source.encodeTyped({ profile: 'prod', source: 'src/main.ts' });

    expect(target.forwardValues(source.decodeValues(sourceInput.value))).toEqual({
      graph: target.codec.graph,
      value: v.record([v.string('src/main.ts'), v.string('prod')]),
    });
  });

  it('requires compatible schema shapes when forwarding canonical values', () => {
    const source = new CanonicalInputModel([
      { name: 'profile', aliases: ['p'], codec: stringCodec },
    ]);
    const values = source.decodeValues(source.encode({ profile: 'prod' }));
    const incompatible = new CanonicalInputModel([
      { name: 'p', aliases: [], codec: compileSchema(z.boolean()) },
    ]);
    const missing = new CanonicalInputModel([{ name: 'tenant', aliases: [], codec: stringCodec }]);

    expect(() => incompatible.forwardValues(values)).toThrow(/incompatible schema/);
    expect(() => missing.forwardValues(values)).toThrow(
      /missing canonical tool input field `tenant`/,
    );
  });

  it('validates standalone canonical record schemas and graph merges', () => {
    expect(
      () =>
        new CanonicalInputModel([
          { name: 'same', aliases: [], codec: stringCodec },
          { name: 'same', aliases: [], codec: stringCodec },
        ]),
    ).toThrowError(expect.objectContaining({ code: 'ill-formed-schema' }));

    const stringRef: FluentCodec = {
      ...stringCodec,
      graph: {
        defs: new Map([['shared', { body: t.string() }]]),
        root: t.ref('shared'),
      },
    };
    const boolRef: FluentCodec = {
      ...compileSchema(z.boolean()),
      graph: {
        defs: new Map([['shared', { body: t.bool() }]]),
        root: t.ref('shared'),
      },
    };
    expect(
      () =>
        new CanonicalInputModel([
          { name: 'first', aliases: [], codec: stringRef },
          { name: 'second', aliases: [], codec: boolRef },
        ]),
    ).toThrowError(expect.objectContaining({ code: 'schema-conflict' }));
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

  it.each(['source validation', 'value encoding'] as const)(
    'does not swallow an unexpected exception from %s while probing value-is codecs',
    (stage) => {
      const failure = new Error(`${stage} failed unexpectedly`);
      const base = compileSchema(z.string());
      const codec: FluentCodec =
        stage === 'source validation'
          ? {
              ...base,
              sourceSchema: {
                '~standard': {
                  version: 1,
                  vendor: 'throwing-test-schema',
                  validate: () => {
                    throw failure;
                  },
                },
              },
            }
          : {
              ...base,
              toValue: () => {
                throw failure;
              },
            };
      const tool = new ExtendedToolType(
        '1.0.0',
        command('unexpected', {
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
            constraints: [
              {
                tag: 'requires-all',
                refs: [
                  {
                    tag: 'value-is',
                    name: 'value',
                    value: { tag: 'deferred', value: 'literal' },
                  },
                ],
              },
            ],
          }),
        }),
      );

      let thrown: unknown;
      try {
        normalizeExtendedTool(tool);
      } catch (error) {
        thrown = error;
      }
      expect(thrown).toBe(failure);
    },
  );

  it('tries a peeled value-is codec after the whole list codec rejects a scalar', () => {
    const codec = listCodec(stringCodec);
    const tool = new ExtendedToolType(
      '1.0.0',
      command('list-value', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'values',
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
                  name: 'values',
                  value: { tag: 'deferred', value: 'needle' },
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
    expect(constraint.refs[0]).toMatchObject({
      tag: 'value-is',
      value: {
        tag: 'resolved',
        value: 'needle',
        schemaValue: { tag: 'string', value: 'needle' },
      },
    });
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

  it('prefers a whole collected value before trying its element codec', () => {
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
                  value: { tag: 'deferred', value: ['whole'] },
                },
              ],
            },
          ],
        }),
      }),
    );

    const constraint = normalizeExtendedTool(tool).root.body?.constraints[0];
    if (constraint?.tag !== 'requires-all') throw new Error('unexpected constraint');
    const ref = constraint.refs[0];
    expect(ref).toMatchObject({
      tag: 'value-is',
      value: { tag: 'resolved', schemaValue: v.list([v.string('whole')]) },
    });
  });

  it('allows value-is to compare a map value and a tail element', () => {
    const mapCodec = compileSchema(KeyValue(z.number()));
    const tool = new ExtendedToolType(
      '1.0.0',
      command('compare', {
        body: body({
          positionals: {
            fixed: [
              {
                name: 'entries',
                doc: emptyDoc(),
                codec: mapCodec,
                required: true,
                acceptsStdio: false,
              },
            ],
            tail: {
              name: 'items',
              doc: emptyDoc(),
              itemCodec: stringCodec,
              min: 0,
              verbatim: false,
              acceptsStdio: false,
            },
          },
          constraints: [
            {
              tag: 'requires-all',
              refs: [
                { tag: 'value-is', name: 'entries', value: { tag: 'deferred', value: 42 } },
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

    const constraint = normalizeExtendedTool(tool).root.body?.constraints[0];
    if (constraint?.tag !== 'requires-all') throw new Error('unexpected constraint');
    expect(constraint.refs).toMatchObject([
      { value: { tag: 'resolved', schemaValue: v.f64(42) } },
      { value: { tag: 'resolved', schemaValue: v.string('needle') } },
    ]);
  });

  it('does not peel more than one supported collection layer', () => {
    const codec = compileSchema(z.array(z.array(z.string())));
    const tool = new ExtendedToolType(
      '1.0.0',
      command('nested', {
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
                  value: { tag: 'deferred', value: 'too-deep' },
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
});
