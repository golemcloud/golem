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

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import {
  KeyValue,
  c,
  command,
  err,
  ok,
  toolDefinition,
  type ToolImplementation,
} from '../src/fluent';
import { compileSchema } from '../src/fluent/schema/adapter';
import { ToolRegistry } from '../src/internal/registry/toolRegistry';
import { CanonicalInputModel } from '../src/internal/tool';
import { t, typedSchemaValueFromWit, typedSchemaValueToWit, v } from '../src/internal/schema-model';
import { tool } from '../src';

beforeEach(() => {
  ToolRegistry.clearForTests();
});

describe('fluent tool registration', () => {
  it('keeps definitions inert until implement is called', () => {
    toolDefinition('inert').body((body) => body.returns(z.void()));

    expect(ToolRegistry.getRegisteredTools()).toEqual([]);
    expect(ToolRegistry.get('inert')).toBeUndefined();
    expect(ToolRegistry.getRegistrationErrors()).toEqual([]);
  });

  it('atomically registers metadata, the extended model, and local command bindings', () => {
    const rootHandler = async () => ok(undefined);
    const remoteHandler = async () => ok('remote');
    const addHandler = async () => ok(undefined);
    const definition = toolDefinition('git')
      .version('2.0.0')
      .body((body) => body.returns(z.void()))
      .command('remote', (remote) =>
        remote
          .body((body) => body.returns(z.string()))
          .command('add', (add) => add.body((body) => body.returns(z.void()))),
      );

    expect(
      definition.implement({
        git: rootHandler,
        remote: command(remoteHandler, { add: addHandler }),
      }),
    ).toEqual({ name: 'git' });

    const registered = ToolRegistry.get('git');
    expect(registered?.encoded.version).toBe('2.0.0');
    expect(registered?.encoded.commands.nodes.map((node) => node.name)).toEqual([
      'git',
      'remote',
      'add',
    ]);
    expect(registered?.extended.toolName).toBe('git');
    expect(registered?.runtime.bindings.map((binding) => binding.commandPath)).toEqual([
      [],
      ['remote'],
      ['remote', 'add'],
    ]);
    expect(registered?.runtime.bindings.map((binding) => binding.handler)).toEqual([
      rootHandler,
      remoteHandler,
      addHandler,
    ]);
    expect(registered?.runtime.subtreeForwards).toEqual([]);
  });

  it('keeps grafted child ownership separate and forwards through its independent registration', async () => {
    const context = { principal: 'caller' };
    let received: { input: unknown; context: unknown } | undefined;
    const remote = toolDefinition('remote')
      .aliases('rmt')
      .command('add', (add) =>
        add.body((body) => body.positional('name', z.string()).returns(z.string())),
      );
    const git = toolDefinition('git')
      .global('git-dir', z.string(), { required: true })
      .command('remote', remote);

    git.implement({});

    const registeredGit = ToolRegistry.get('git');
    const commandNode = registeredGit?.extended.commandByPath(['remote', 'add']);
    if (!registeredGit || !commandNode) throw new Error('git subtree was not registered');
    const input = registeredGit.extended
      .canonicalInputModel(commandNode)
      .encodeTyped({ 'git-dir': '.git', name: 'origin' });

    expect(registeredGit.runtime.bindings).toEqual([]);
    expect(registeredGit.runtime.subtreeForwards).toEqual([
      { pathPrefix: ['remote'], childToolName: 'remote' },
    ]);
    expect(ToolRegistry.getRegisteredTools().map((tool) => tool.commands.nodes[0].name)).toEqual([
      'git',
    ]);
    await expect(registeredGit.invoker(['remote', 'add'], input, context)).rejects.toEqual({
      tag: 'invalid-tool-name',
      val: 'remote',
    });

    remote.implement({
      add: async (handlerInput, handlerContext) => {
        received = { input: handlerInput, context: handlerContext };
        return ok(`added ${handlerInput.name}`);
      },
    });

    await expect(registeredGit.invoker(['rmt', 'add'], input, context)).resolves.toEqual(
      ok('added origin'),
    );
    expect(received).toEqual({ input: { name: 'origin' }, context });
    expect(ToolRegistry.getRegisteredTools().map((tool) => tool.commands.nodes[0].name)).toEqual([
      'git',
      'remote',
    ]);
  });

  it('forwards an inherited canonical value to a de-projected child field', async () => {
    let received: unknown;
    const remote = toolDefinition('remote')
      .global('config', z.string(), { required: true })
      .body((body) =>
        body.constraint(c.requiresAll([c.valueIs('config', 'prod')])).returns(z.string()),
      )
      .command('show', (show) => show.body((body) => body.returns(z.string())));
    const git = toolDefinition('git')
      .global('profile', z.string(), {
        aliases: ['config'],
        required: true,
        doc: 'Parent profile',
      })
      .command('remote', remote);

    git.implement({});
    remote.implement({
      remote: async (input) => {
        received = input;
        return ok(input.config);
      },
      show: async (input) => {
        received = input;
        return ok(input.config);
      },
    });

    const registeredGit = ToolRegistry.get('git');
    const registeredRemote = ToolRegistry.get('remote');
    const remoteNode = registeredGit?.extended.commandByPath(['remote']);
    const showNode = registeredGit?.extended.commandByPath(['remote', 'show']);
    if (!registeredGit || !registeredRemote || !remoteNode || !showNode) {
      throw new Error('de-projected tools were not registered');
    }

    expect(remoteNode.globals.options).toEqual([]);
    expect(remoteNode.body?.constraints).toMatchObject([
      {
        refs: [
          {
            tag: 'value-is',
            name: 'config',
            value: { tag: 'resolved', schemaValue: { tag: 'string', value: 'prod' } },
          },
        ],
      },
    ]);
    expect(
      registeredGit.extended.canonicalInputFields(remoteNode).map((field) => field.name),
    ).toEqual(['profile']);
    expect(
      registeredGit.extended.canonicalInputFields(showNode).map((field) => field.name),
    ).toEqual(['profile']);
    expect(
      registeredRemote.extended
        .canonicalInputFields(registeredRemote.extended.root)
        .map((field) => field.name),
    ).toEqual(['config']);
    expect(registeredGit.encoded.commands.nodes[0].globals.options[0]).toMatchObject({
      long: 'profile',
      aliases: ['config'],
      doc: { summary: 'Parent profile' },
    });
    expect(registeredGit.encoded.commands.nodes[1].globals.options).toEqual([]);

    const rootInput = registeredGit.extended
      .canonicalInputModel(remoteNode)
      .encodeTyped({ profile: 'prod' });
    await expect(registeredGit.invoker(['remote'], rootInput, {})).resolves.toEqual(ok('prod'));
    expect(received).toEqual({ config: 'prod' });

    const nestedInput = registeredGit.extended
      .canonicalInputModel(showNode)
      .encodeTyped({ profile: 'dev' });
    await expect(registeredGit.invoker(['remote', 'show'], nestedInput, {})).resolves.toEqual(
      ok('dev'),
    );
    expect(received).toEqual({ config: 'dev' });
  });

  it('adapts an optional ancestor carrier for a required standalone child field', async () => {
    let received: unknown;
    const child = toolDefinition('child')
      .global('config', z.string(), { required: true })
      .body((body) => body.returns(z.void()));
    const parent = toolDefinition('parent')
      .global('profile', z.string(), { aliases: ['config'], optionalScalar: true })
      .command('child', child);

    parent.implement({});
    child.implement({
      child: async (input) => {
        received = input.config;
        return ok(undefined);
      },
    });

    const registered = ToolRegistry.get('parent');
    const childNode = registered?.extended.commandByPath(['child']);
    if (!registered || !childNode) throw new Error('child graft was not registered');
    const model = registered.extended.canonicalInputModel(childNode);

    await expect(
      registered.invoker(['child'], model.encodeTyped({ profile: 'production' }), {}),
    ).resolves.toEqual(ok(undefined));
    expect(received).toBe('production');

    received = undefined;
    await expect(
      registered.invoker(['child'], model.encodeTyped({ profile: undefined }), {}),
    ).rejects.toMatchObject({ tag: 'invalid-input' });
    expect(received).toBeUndefined();
  });

  it('projects inherited aliases through more than one separately registered graft', async () => {
    let received: unknown;
    const leaf = toolDefinition('leaf')
      .global('config', z.string(), { required: true })
      .body((body) => body.returns(z.string()));
    const middle = toolDefinition('middle')
      .global('profile', z.string(), { aliases: ['config'], required: true })
      .command('leaf', leaf);
    const root = toolDefinition('root')
      .global('tenant', z.string(), {
        aliases: ['profile', 'config'],
        required: true,
      })
      .command('middle', middle);

    root.implement({});
    middle.implement({});
    leaf.implement({
      leaf: async (input) => {
        received = input;
        return ok(input.config);
      },
    });

    const registered = ToolRegistry.get('root');
    const leafNode = registered?.extended.commandByPath(['middle', 'leaf']);
    if (!registered || !leafNode) throw new Error('nested graft was not registered');
    expect(registered.extended.canonicalInputFields(leafNode).map((field) => field.name)).toEqual([
      'tenant',
    ]);
    expect(registered.encoded.commands.nodes[1].globals.options).toEqual([]);
    expect(registered.encoded.commands.nodes[2].globals.options).toEqual([]);

    const input = registered.extended
      .canonicalInputModel(leafNode)
      .encodeTyped({ tenant: 'production' });
    await expect(registered.invoker(['middle', 'leaf'], input, {})).resolves.toEqual(
      ok('production'),
    );
    expect(received).toEqual({ config: 'production' });
  });

  it('does not propagate aliases from a de-projected declaration into nested grafts', async () => {
    let received: unknown;
    const leaf = toolDefinition('leaf')
      .global('config', z.string(), { required: true })
      .body((body) => body.returns(z.string()));
    const middle = toolDefinition('middle')
      .global('profile', z.string(), { aliases: ['config'], required: true })
      .command('leaf', leaf);
    const root = toolDefinition('root')
      .global('tenant', z.string(), { aliases: ['profile'], required: true })
      .command('middle', middle);

    root.implement({});
    middle.implement({});
    leaf.implement({
      leaf: async (input) => {
        received = input;
        return ok(input.config);
      },
    });

    const registered = ToolRegistry.get('root');
    const leafNode = registered?.extended.commandByPath(['middle', 'leaf']);
    if (!registered || !leafNode) throw new Error('nested graft was not registered');
    expect(registered.extended.canonicalInputFields(leafNode).map((field) => field.name)).toEqual([
      'tenant',
      'config',
    ]);

    const input = registered.extended
      .canonicalInputModel(leafNode)
      .encodeTyped({ tenant: 'production', config: 'leaf-production' });
    await expect(registered.invoker(['middle', 'leaf'], input, {})).resolves.toEqual(
      ok('leaf-production'),
    );
    expect(received).toEqual({ config: 'leaf-production' });
  });

  it('reconstructs a child-native value when equal wire graphs use different codecs', async () => {
    let received: unknown;
    const child = toolDefinition('child')
      .global('labels', KeyValue(z.string()), { required: true })
      .body((body) => body.returns(z.void()));
    const parent = toolDefinition('parent')
      .global('labels', z.record(z.string(), z.string()), { required: true })
      .command('child', child);

    parent.implement({});
    child.implement({
      child: async (input) => {
        received = input.labels;
        return ok(undefined);
      },
    });

    const registered = ToolRegistry.get('parent');
    const childNode = registered?.extended.commandByPath(['child']);
    if (!registered || !childNode) throw new Error('child graft was not registered');
    expect(childNode.globals.options).toEqual([]);
    expect(registered.extended.canonicalInputFields(childNode).map((field) => field.name)).toEqual([
      'labels',
    ]);

    const input = registered.extended
      .canonicalInputModel(childNode)
      .encodeTyped({ labels: { environment: 'production' } });
    await expect(registered.invoker(['child'], input, {})).resolves.toEqual(ok(undefined));
    expect(received).toEqual(new Map([['environment', 'production']]));
  });

  it('forwards values accepted by structurally compatible restricted graft fields', async () => {
    let received: unknown;
    const child = toolDefinition('child')
      .global('config', z.number().min(1), { required: true })
      .body((body) => body.returns(z.void()));
    const parent = toolDefinition('parent')
      .global('config', z.number().max(10), { required: true })
      .command('child', child);

    parent.implement({});
    child.implement({
      child: async (input) => {
        received = input.config;
        return ok(undefined);
      },
    });

    const registered = ToolRegistry.get('parent');
    const childNode = registered?.extended.commandByPath(['child']);
    if (!registered || !childNode) throw new Error('child graft was not registered');
    expect(childNode.globals.options).toEqual([]);

    const input = registered.extended.canonicalInputModel(childNode).encodeTyped({ config: 5 });
    await expect(registered.invoker(['child'], input, {})).resolves.toEqual(ok(undefined));
    expect(received).toBe(5);
  });

  it('rejects incompatible canonical field shapes when forwarding to a registered child', async () => {
    const declaredRemote = toolDefinition('remote').body((body) =>
      body.positional('value', z.string()).returns(z.void()),
    );
    const git = toolDefinition('git').command('remote', declaredRemote);
    git.implement({});
    toolDefinition('remote')
      .body((body) => body.positional('value', z.boolean()).returns(z.void()))
      .implement({ remote: async () => ok(undefined) });

    const registeredGit = ToolRegistry.get('git');
    const commandNode = registeredGit?.extended.commandByPath(['remote']);
    if (!registeredGit || !commandNode) throw new Error('git subtree was not registered');
    const input = registeredGit.extended
      .canonicalInputModel(commandNode)
      .encodeTyped({ value: 'text' });

    await expect(registeredGit.invoker(['remote'], input, {})).rejects.toEqual({
      tag: 'invalid-input',
      val: expect.stringContaining('incompatible schema for forwarded field `value`'),
    });
  });

  it('rejects a grafted dispatcher as an invalid command path', async () => {
    const remote = toolDefinition('remote').command('add', (add) =>
      add.body((body) => body.returns(z.void())),
    );
    const git = toolDefinition('git').command('remote', remote);
    git.implement({});

    const registeredGit = ToolRegistry.get('git');
    const dispatcher = registeredGit?.extended.commandByPath(['remote'], false);
    if (!registeredGit || !dispatcher) throw new Error('git subtree was not registered');
    const input = registeredGit.extended.canonicalInputModel(dispatcher).encodeTyped({});

    await expect(registeredGit.invoker(['remote'], input, {})).rejects.toEqual({
      tag: 'invalid-command-path',
      val: ['remote'],
    });

    remote.implement({ add: async () => ok(undefined) });

    await expect(registeredGit.invoker(['remote'], input, {})).rejects.toEqual({
      tag: 'invalid-command-path',
      val: ['remote'],
    });
  });

  it('rejects input values that do not conform to the command schema', async () => {
    const definition = toolDefinition('flag-tool').body((body) =>
      body.flag('force').returns(z.void()),
    );
    let received: unknown;
    definition.implement({
      'flag-tool': async (args) => {
        received = args.force;
        return ok(undefined);
      },
    });

    const registered = ToolRegistry.get('flag-tool');
    if (!registered) throw new Error('flag tool was not registered');
    const incompatibleInput = new CanonicalInputModel([
      {
        name: 'force',
        aliases: [],
        codec: compileSchema(z.string()),
      },
    ]).encodeTyped({ force: 'yes' });

    await expect(registered.invoker([], incompatibleInput, {})).rejects.toMatchObject({
      tag: 'invalid-input',
    });
    expect(received).toBeUndefined();
  });

  it('accepts the canonical option carrier for an omitted optional argument', async () => {
    const definition = toolDefinition('optional-tool').body((body) =>
      body.option('label', z.string()).returns(z.void()),
    );
    let received: unknown;
    definition.implement({
      'optional-tool': async (args) => {
        received = args.label;
        return ok(undefined);
      },
    });

    const registered = ToolRegistry.get('optional-tool');
    const commandNode = registered?.extended.commandByPath([]);
    if (!registered || !commandNode) throw new Error('optional tool was not registered');
    const input = registered.extended
      .canonicalInputModel(commandNode)
      .encodeTyped({ label: undefined });

    await expect(registered.invoker([], input, {})).resolves.toEqual(ok(undefined));
    expect(received).toBeUndefined();
  });

  it('rejects a missing canonical carrier for a present optional-of-option argument', async () => {
    const definition = toolDefinition('nested-optional-tool').body((body) =>
      body.option('label', z.string().optional()).returns(z.void()),
    );
    let called = false;
    definition.implement({
      'nested-optional-tool': async () => {
        called = true;
        return ok(undefined);
      },
    });

    const registered = ToolRegistry.get('nested-optional-tool');
    const commandNode = registered?.extended.commandByPath([]);
    if (!registered || !commandNode) throw new Error('nested optional tool was not registered');
    const inputModel = registered.extended.canonicalInputModel(commandNode);
    expect(inputModel.encodeTyped({ label: 'present' }).value).toEqual(
      v.record([v.option(v.option(v.string('present')))]),
    );

    const nonCanonicalInput = {
      graph: inputModel.codec.graph,
      value: v.record([v.option(v.string('present'))]),
    };
    await expect(registered.invoker([], nonCanonicalInput, {})).rejects.toMatchObject({
      tag: 'invalid-input',
    });
    expect(called).toBe(false);
  });

  it('rejects non-canonical values for required fields without an outer option carrier', async () => {
    const definition = toolDefinition('literal-tool').body((body) =>
      body.option('mode', z.literal('right').optional(), { required: true }).returns(z.void()),
    );
    let called = false;
    definition.implement({
      'literal-tool': async () => {
        called = true;
        return ok(undefined);
      },
    });

    const registered = ToolRegistry.get('literal-tool');
    const commandNode = registered?.extended.commandByPath([]);
    if (!registered || !commandNode) throw new Error('literal tool was not registered');
    const inputModel = registered.extended.canonicalInputModel(commandNode);
    const nonCanonicalInput = {
      graph: inputModel.codec.graph,
      value: v.record([v.option(v.string('wrong'))]),
    };

    await expect(registered.invoker([], nonCanonicalInput, {})).rejects.toMatchObject({
      tag: 'invalid-input',
    });
    expect(called).toBe(false);
  });

  it('preserves a separately registered child handler receiver when forwarding', async () => {
    const remote = toolDefinition('remote').command('add', (add) =>
      add.body((body) => body.returns(z.string())),
    );
    const git = toolDefinition('git').command('remote', remote);
    git.implement({});

    class RemoteImplementation {
      readonly #prefix = 'added';

      async add() {
        return ok(this.#prefix);
      }
    }

    const implementation: ToolImplementation<typeof remote> = new RemoteImplementation();
    remote.implement(implementation);

    const registeredGit = ToolRegistry.get('git');
    const commandNode = registeredGit?.extended.commandByPath(['remote', 'add']);
    if (!registeredGit || !commandNode) throw new Error('git subtree was not registered');
    const input = registeredGit.extended.canonicalInputModel(commandNode).encodeTyped({});

    await expect(registeredGit.invoker(['remote', 'add'], input, {})).resolves.toEqual(ok('added'));
  });

  it('registers a structurally valid implementation with prototype-defined handlers', () => {
    const definition = toolDefinition('prototype-tool').command('leaf', (leaf) =>
      leaf.body((body) => body.returns(z.void())),
    );

    class Implementation {
      async leaf() {
        return ok(undefined);
      }
    }

    const implementation: ToolImplementation<typeof definition> = new Implementation();
    definition.implement(implementation);

    expect(ToolRegistry.getRuntime('prototype-tool')?.bindings).toHaveLength(1);
    expect(ToolRegistry.getRegistrationError('prototype-tool')).toBeUndefined();
  });

  it('preserves the receiver of prototype handlers wrapped by command()', async () => {
    const definition = toolDefinition('wrapped-prototype-tool').command('group', (group) =>
      group.command('leaf', (leaf) => leaf.body((body) => body.returns(z.string()))),
    );

    class GroupImplementation {
      readonly #value = 'wrapped receiver';

      async leaf() {
        return ok(this.#value);
      }
    }

    const group = new GroupImplementation();
    definition.implement({ group: command(group) });

    const registered = ToolRegistry.get('wrapped-prototype-tool');
    const commandNode = registered?.extended.commandByPath(['group', 'leaf']);
    if (!registered || !commandNode) throw new Error('wrapped prototype tool was not registered');
    const input = registered.extended.canonicalInputModel(commandNode).encodeTyped({});

    await expect(registered.invoker(['group', 'leaf'], input, {})).resolves.toEqual(
      ok('wrapped receiver'),
    );
  });

  it('preserves non-enumerable child handlers when command wraps an implementation object', () => {
    const definition = toolDefinition('non-enumerable-tool').command('group', (group) =>
      group.command('leaf', (leaf) => leaf.body((body) => body.returns(z.void()))),
    );
    const children = {} as { leaf: () => Promise<ReturnType<typeof ok<undefined>>> };
    Object.defineProperty(children, 'leaf', {
      value: async () => ok(undefined),
      enumerable: false,
    });

    definition.implement({ group: command(children) });

    expect(ToolRegistry.getRuntime('non-enumerable-tool')?.bindings).toHaveLength(1);
    expect(ToolRegistry.getRegistrationError('non-enumerable-tool')).toBeUndefined();
  });

  it('binds a same-named child below a dispatcher root', () => {
    const handler = async () => ok(undefined);
    const definition = toolDefinition('echo').command('echo', (child) =>
      child.body((body) => body.returns(z.void())),
    );

    definition.implement({ echo: handler });

    expect(ToolRegistry.getRegistrationError('echo')).toBeUndefined();
    expect(ToolRegistry.getRuntime('echo')?.bindings).toMatchObject([
      { commandPath: ['echo'], handler },
    ]);
  });

  it('rejects a missing root handler whose name exists on Object.prototype', () => {
    const definition = toolDefinition('constructor').body((body) => body.returns(z.void()));

    definition.implement({} as ToolImplementation<typeof definition>);

    expect(ToolRegistry.get('constructor')).toBeUndefined();
    expect(ToolRegistry.getRegistrationError('constructor')).toEqual([
      expect.stringContaining('implementation must be a function'),
    ]);
  });

  it('discovers registered tools deterministically by root name', () => {
    toolDefinition('zeta')
      .body((body) => body.returns(z.void()))
      .implement({ zeta: async () => ok(undefined) });
    toolDefinition('alpha')
      .body((body) => body.returns(z.void()))
      .implement({ alpha: async () => ok(undefined) });

    expect(ToolRegistry.getRegisteredTools().map((tool) => tool.commands.nodes[0].name)).toEqual([
      'alpha',
      'zeta',
    ]);
  });

  it('defers duplicate implementation and duplicate tool-name diagnostics', () => {
    const firstHandler = async () => ok(undefined);
    const definition = toolDefinition('duplicate').body((body) => body.returns(z.void()));
    definition.implement({ duplicate: firstHandler });
    expect(() => definition.implement({ duplicate: firstHandler })).not.toThrow();

    toolDefinition('duplicate')
      .body((body) => body.returns(z.void()))
      .implement({ duplicate: async () => ok(undefined) });

    expect(ToolRegistry.getRegistrationError('duplicate')).toEqual([
      expect.stringContaining('implement() was called more than once'),
      expect.stringContaining('already registered'),
    ]);
    expect(ToolRegistry.getRuntime('duplicate')?.bindings[0].handler).toBe(firstHandler);
    expect(ToolRegistry.getRegisteredTools()).toHaveLength(1);
  });

  it('reserves a tool name before reading implementation properties', () => {
    const outerHandler = async () => ok(undefined);
    const innerHandler = async () => ok(undefined);
    const outer = toolDefinition('reentrant').body((body) => body.returns(z.void()));
    const inner = toolDefinition('reentrant').body((body) => body.returns(z.void()));
    let entered = false;
    const implementation = {} as ToolImplementation<typeof outer>;
    Object.defineProperty(implementation, 'reentrant', {
      get() {
        if (!entered) {
          entered = true;
          inner.implement({ reentrant: innerHandler });
        }
        return outerHandler;
      },
    });

    outer.implement(implementation);

    expect(ToolRegistry.getRuntime('reentrant')?.bindings[0]?.handler).toBe(outerHandler);
    expect(ToolRegistry.getRegistrationError('reentrant')).toEqual([
      expect.stringContaining('already registered'),
    ]);
  });

  it('does not leave partial state when finalization or binding fails', () => {
    const invalidDefinition = toolDefinition('Invalid').body((body) => body.returns(z.void()));
    invalidDefinition.implement({ Invalid: async () => ok(undefined) });

    const missingHandlerDefinition = toolDefinition('missing-handler').body((body) =>
      body.returns(z.void()),
    );
    missingHandlerDefinition.implement({} as ToolImplementation<typeof missingHandlerDefinition>);

    expect(ToolRegistry.get('Invalid')).toBeUndefined();
    expect(ToolRegistry.get('missing-handler')).toBeUndefined();
    expect(ToolRegistry.getRegistrationError('Invalid')).toEqual([
      expect.stringContaining('invalid command name'),
    ]);
    expect(ToolRegistry.getRegistrationError('missing-handler')).toEqual([
      expect.stringContaining('implementation must be a function'),
    ]);
  });

  it('clears registrations and diagnostics for test isolation', () => {
    const definition = toolDefinition('reset').body((body) => body.returns(z.void()));
    definition.implement({ reset: async () => ok(undefined) });
    definition.implement({ reset: async () => ok(undefined) });

    ToolRegistry.clearForTests();

    expect(ToolRegistry.getRegisteredTools()).toEqual([]);
    expect(ToolRegistry.getRegistrationErrors()).toEqual([]);
  });
});

describe('tool guest exports', () => {
  it('discovers registered descriptors and looks them up by root name', () => {
    toolDefinition('zeta')
      .body((body) => body.returns(z.void()))
      .implement({ zeta: async () => ok(undefined) });
    toolDefinition('alpha')
      .body((body) => body.returns(z.void()))
      .implement({ alpha: async () => ok(undefined) });

    expect(tool.discoverTools()).toMatchObject([
      { commands: { nodes: [{ name: 'alpha' }] } },
      { commands: { nodes: [{ name: 'zeta' }] } },
    ]);
    expect(tool.getTool('zeta')).toBe(ToolRegistry.getTool('zeta'));
    let missing: unknown;
    try {
      tool.getTool('missing');
    } catch (error) {
      missing = error;
    }
    expect(missing).toEqual({
      tag: 'invalid-tool-name',
      val: 'missing',
    });
  });

  it('reports deferred registration diagnostics through discovery', () => {
    toolDefinition('Invalid')
      .body((body) => body.returns(z.void()))
      .implement({ Invalid: async () => ok(undefined) });

    let discoveryError: unknown;
    try {
      tool.discoverTools();
    } catch (error) {
      discoveryError = error;
    }
    expect(discoveryError).toEqual({
      tag: 'invalid-result',
      val: expect.stringContaining('Tool "Invalid"'),
    });
  });

  it('decodes canonical input, injects the principal, and encodes a local result', async () => {
    let received: { input: unknown; principalTag: string } | undefined;
    toolDefinition('echo')
      .body((body) => body.positional('message', z.string()).returns(z.string()))
      .implement({
        echo: async (input, context) => {
          received = { input, principalTag: context.principal.tag };
          return ok(input.message.toUpperCase());
        },
      });
    const registered = ToolRegistry.get('echo');
    const commandNode = registered?.extended.commandByPath([]);
    if (!registered || !commandNode) throw new Error('echo tool was not registered');
    const input = registered.extended
      .canonicalInputModel(commandNode)
      .encodeTyped({ message: 'hello' });

    const result = await tool.invoke('echo', [], typedSchemaValueToWit(input), undefined, {
      tag: 'anonymous',
    });

    expect(received).toEqual({ input: { message: 'hello' }, principalTag: 'anonymous' });
    expect(result.stdout).toBeUndefined();
    expect(result.result).toBeDefined();
    const decoded = typedSchemaValueFromWit(result.result!);
    expect(commandNode.body?.result?.codec.fromValue(decoded.value)).toBe('HELLO');
  });

  it('maps unknown tools, command paths, and invalid canonical values to exact tool errors', async () => {
    toolDefinition('echo')
      .body((body) => body.positional('message', z.string()).returns(z.void()))
      .implement({ echo: async () => ok(undefined) });
    const registered = ToolRegistry.get('echo');
    const commandNode = registered?.extended.commandByPath([]);
    if (!registered || !commandNode) throw new Error('echo tool was not registered');
    const inputModel = registered.extended.canonicalInputModel(commandNode);
    const validInput = typedSchemaValueToWit(inputModel.encodeTyped({ message: 'hello' }));

    await expect(
      tool.invoke('missing', [], validInput, undefined, { tag: 'anonymous' }),
    ).rejects.toEqual({ tag: 'invalid-tool-name', val: 'missing' });
    await expect(
      tool.invoke('echo', ['missing'], validInput, undefined, { tag: 'anonymous' }),
    ).rejects.toEqual({ tag: 'invalid-command-path', val: ['missing'] });

    const invalidValue = typedSchemaValueToWit({
      graph: inputModel.codec.graph,
      value: v.record([v.bool(true)]),
    });
    await expect(
      tool.invoke('echo', [], invalidValue, undefined, { tag: 'anonymous' }),
    ).rejects.toMatchObject({ tag: 'invalid-input' });
  });

  it('encodes declared custom errors with and without payloads', async () => {
    toolDefinition('fallible')
      .body((body) =>
        body.returns(z.string()).error('failed', {
          kind: 'runtime',
          exitCode: 1,
          payload: z.object({ reason: z.string() }),
        }),
      )
      .implement({
        fallible: async () => err('failed', { reason: 'nope' }),
      });
    const fallible = ToolRegistry.get('fallible');
    const fallibleCommand = fallible?.extended.commandByPath([]);
    if (!fallible || !fallibleCommand) throw new Error('fallible tool was not registered');
    const fallibleInput = typedSchemaValueToWit(
      fallible.extended.canonicalInputModel(fallibleCommand).encodeTyped({}),
    );

    const customError = await tool
      .invoke('fallible', [], fallibleInput, undefined, { tag: 'anonymous' })
      .then(
        () => undefined,
        (error: unknown) => error,
      );
    expect(customError).toMatchObject({ tag: 'custom-error' });
    const payload = typedSchemaValueFromWit(
      (customError as { val: Parameters<typeof typedSchemaValueFromWit>[0] }).val,
    );
    expect(fallibleCommand.body?.errors[0].payloadCodec?.fromValue(payload.value)).toEqual({
      reason: 'nope',
    });

    toolDefinition('payloadless')
      .body((body) => body.returns(z.void()).error('not-found', { kind: 'runtime', exitCode: 1 }))
      .implement({ payloadless: async () => err('not-found') });
    const payloadless = ToolRegistry.get('payloadless');
    const payloadlessCommand = payloadless?.extended.commandByPath([]);
    if (!payloadless || !payloadlessCommand) {
      throw new Error('payloadless tool was not registered');
    }
    const payloadlessInput = typedSchemaValueToWit(
      payloadless.extended.canonicalInputModel(payloadlessCommand).encodeTyped({}),
    );

    const payloadlessError = await tool
      .invoke('payloadless', [], payloadlessInput, undefined, { tag: 'anonymous' })
      .then(
        () => undefined,
        (error: unknown) => error,
      );
    expect(payloadlessError).toMatchObject({ tag: 'custom-error' });
    const unitPayload = typedSchemaValueFromWit(
      (payloadlessError as { val: Parameters<typeof typedSchemaValueFromWit>[0] }).val,
    );
    expect(unitPayload).toEqual({
      graph: { defs: new Map(), root: t.tuple([]) },
      value: v.tuple([]),
    });
  });

  it('maps invalid success and declared-error outcomes to invalid-result', async () => {
    toolDefinition('invalid-success-type')
      .body((body) => body.returns(z.string()))
      .implement({ 'invalid-success-type': async () => ok(42 as never) });
    toolDefinition('invalid-unit-success')
      .body((body) => body.returns(z.void()))
      .implement({ 'invalid-unit-success': async () => ok('unexpected' as never) });
    toolDefinition('missing-success-value')
      .body((body) => body.returns(z.string()))
      .implement({
        'missing-success-value': async () => ({ tag: 'ok' }) as never,
      });
    toolDefinition('undeclared-error')
      .body((body) => body.returns(z.void()).error('declared', { kind: 'runtime', exitCode: 1 }))
      .implement({
        'undeclared-error': async () => ({ tag: 'err', name: 'other', hasPayload: false }) as never,
      });
    toolDefinition('missing-error-payload')
      .body((body) =>
        body.returns(z.void()).error('failed', {
          kind: 'runtime',
          exitCode: 1,
          payload: z.object({ reason: z.string() }),
        }),
      )
      .implement({
        'missing-error-payload': async () =>
          ({ tag: 'err', name: 'failed', hasPayload: false }) as never,
      });
    toolDefinition('invalid-error-payload')
      .body((body) =>
        body.returns(z.void()).error('failed', {
          kind: 'runtime',
          exitCode: 1,
          payload: z.object({ reason: z.string() }),
        }),
      )
      .implement({
        'invalid-error-payload': async () => err('failed', { reason: 42 as never }),
      });
    toolDefinition('unexpected-error-payload')
      .body((body) => body.returns(z.void()).error('failed', { kind: 'runtime', exitCode: 1 }))
      .implement({
        'unexpected-error-payload': async () => err('failed', undefined) as never,
      });
    toolDefinition('unknown-outcome')
      .body((body) => body.returns(z.void()))
      .implement({
        'unknown-outcome': async () => ({ tag: 'unknown' }) as never,
      });

    const invoke = async (name: string): Promise<unknown> => {
      const registered = ToolRegistry.get(name);
      const commandNode = registered?.extended.commandByPath([]);
      if (!registered || !commandNode) throw new Error(`${name} tool was not registered`);
      const input = typedSchemaValueToWit(
        registered.extended.canonicalInputModel(commandNode).encodeTyped({}),
      );
      return await tool.invoke(name, [], input, undefined, { tag: 'anonymous' });
    };

    await expect(invoke('invalid-success-type')).rejects.toEqual({
      tag: 'invalid-result',
      val: 'tool result: does not match its declared schema',
    });
    await expect(invoke('invalid-unit-success')).rejects.toEqual({
      tag: 'invalid-result',
      val: 'unit tool handler returned a structured result',
    });
    await expect(invoke('missing-success-value')).rejects.toEqual({
      tag: 'invalid-result',
      val: 'tool handler success is missing its value',
    });
    await expect(invoke('undeclared-error')).rejects.toEqual({
      tag: 'invalid-result',
      val: 'tool handler returned undeclared error "other"',
    });
    await expect(invoke('missing-error-payload')).rejects.toEqual({
      tag: 'invalid-result',
      val: 'tool error "failed" requires a payload',
    });
    await expect(invoke('invalid-error-payload')).rejects.toEqual({
      tag: 'invalid-result',
      val: 'tool error "failed" payload: does not match its declared schema',
    });
    await expect(invoke('unexpected-error-payload')).rejects.toEqual({
      tag: 'invalid-result',
      val: 'tool error "failed" does not declare a payload',
    });
    await expect(invoke('unknown-outcome')).rejects.toEqual({
      tag: 'invalid-result',
      val: 'tool handler returned unknown outcome tag "unknown"',
    });
  });

  it('rejects non-canonical handler results', async () => {
    toolDefinition('invalid-result')
      .body((body) => body.returns(z.object({ value: z.string() })))
      .implement({
        'invalid-result': async () => ok({ value: 'valid', extra: true } as never),
      });

    const invalid = ToolRegistry.get('invalid-result');
    const invalidCommand = invalid?.extended.commandByPath([]);
    if (!invalid || !invalidCommand) throw new Error('invalid-result tool was not registered');
    const invalidInput = typedSchemaValueToWit(
      invalid.extended.canonicalInputModel(invalidCommand).encodeTyped({}),
    );

    await expect(
      tool.invoke('invalid-result', [], invalidInput, undefined, { tag: 'anonymous' }),
    ).rejects.toEqual({
      tag: 'invalid-result',
      val: 'tool result: is not canonical for its declared schema',
    });
  });

  it('encodes transformed handler outputs without applying the transform again', async () => {
    toolDefinition('transformed-result')
      .body((body) => body.returns(z.string().transform((value) => `${value}!`)))
      .implement({
        'transformed-result': async () => ok('done!'),
      });
    const registered = ToolRegistry.get('transformed-result');
    const commandNode = registered?.extended.commandByPath([]);
    if (!registered || !commandNode) throw new Error('transformed-result was not registered');
    const input = typedSchemaValueToWit(
      registered.extended.canonicalInputModel(commandNode).encodeTyped({}),
    );

    const result = await tool.invoke('transformed-result', [], input, undefined, {
      tag: 'anonymous',
    });

    expect(result.result).toBeDefined();
    const decoded = typedSchemaValueFromWit(result.result!);
    expect(commandNode.body?.result?.codec.fromValue(decoded.value)).toBe('done!');
  });

  describe('Preview 3 tool streams', () => {
    const invocationInput = (name: string) => {
      const registered = ToolRegistry.get(name);
      const commandNode = registered?.extended.commandByPath([]);
      if (!registered || !commandNode) throw new Error(`${name} tool was not registered`);
      return typedSchemaValueToWit(
        registered.extended.canonicalInputModel(commandNode).encodeTyped({}),
      );
    };

    it('rejects missing required stdin', async () => {
      toolDefinition('required-stdin')
        .body((body) => body.stdin({ required: true }).returns(z.void()))
        .implement({ 'required-stdin': async () => ok(undefined) });

      await expect(
        tool.invoke('required-stdin', [], invocationInput('required-stdin'), undefined, {
          tag: 'anonymous',
        }),
      ).rejects.toEqual({
        tag: 'invalid-input',
        val: 'tool invocation did not contain declared stdin stream',
      });
    });

    it('makes byte input readable by the handler and closes its iterator', async () => {
      const iteratorReturn = vi.fn().mockResolvedValue({ done: true, value: undefined });
      const stdin = {
        [Symbol.asyncIterator]() {
          const bytes = [104, 105];
          return {
            next: vi.fn(async () =>
              bytes.length
                ? { done: false as const, value: bytes.shift()! }
                : { done: true as const, value: undefined },
            ),
            return: iteratorReturn,
          };
        },
      };
      let received: number[] = [];
      toolDefinition('read-stdin')
        .body((body) => body.stdin({ required: true }).returns(z.void()))
        .implement({
          'read-stdin': async (_, context) => {
            const reader = context.stdin.getReader();
            while (true) {
              const chunk = await reader.read();
              if (chunk.done) break;
              received.push(...chunk.value);
            }
            return ok(undefined);
          },
        });

      await expect(
        tool.invoke('read-stdin', [], invocationInput('read-stdin'), stdin, { tag: 'anonymous' }),
      ).resolves.toEqual({ result: undefined, stdout: undefined });
      expect(received).toEqual([104, 105]);
      expect(iteratorReturn).toHaveBeenCalledOnce();
    });

    it('returns handler stdout bytes as an async iterable', async () => {
      toolDefinition('write-stdout')
        .body((body) => body.stdout({ required: true }).returns(z.void()))
        .implement({
          'write-stdout': async (_, context) => {
            const writer = context.stdout.getWriter();
            await writer.write(new Uint8Array([1, 2]));
            await writer.write(new Uint8Array([3]));
            return ok(undefined);
          },
        });

      const result = await tool.invoke(
        'write-stdout',
        [],
        invocationInput('write-stdout'),
        undefined,
        { tag: 'anonymous' },
      );
      const bytes: number[] = [];
      for await (const byte of result.stdout!) bytes.push(byte);
      expect(bytes).toEqual([1, 2, 3]);
    });

    it('errors when stdin yields a value outside the byte range', async () => {
      async function* invalidInput() {
        yield 256;
      }
      toolDefinition('invalid-stdin')
        .body((body) => body.stdin({ required: true }).returns(z.void()))
        .implement({
          'invalid-stdin': async (_, context) => {
            await context.stdin.getReader().read();
            return ok(undefined);
          },
        });

      await expect(
        tool.invoke('invalid-stdin', [], invocationInput('invalid-stdin'), invalidInput(), {
          tag: 'anonymous',
        }),
      ).rejects.toThrow('tool stdin yielded a value outside the byte range');
    });

    it('rejects writes through a retained writer after invocation', async () => {
      let writer: WritableStreamDefaultWriter<Uint8Array> | undefined;
      toolDefinition('retained-writer')
        .body((body) => body.stdout({ required: true }).returns(z.void()))
        .implement({
          'retained-writer': async (_, context) => {
            writer = context.stdout.getWriter();
            await writer.write(new Uint8Array([1]));
            return ok(undefined);
          },
        });

      await tool.invoke('retained-writer', [], invocationInput('retained-writer'), undefined, {
        tag: 'anonymous',
      });
      await expect(writer!.write(new Uint8Array([2]))).rejects.toThrow('tool invocation completed');
    });

    it('propagates the handler abort reason', async () => {
      const reason = new Error('handler aborted stdout');
      toolDefinition('abort-stdout')
        .body((body) => body.stdout({ required: true }).returns(z.void()))
        .implement({
          'abort-stdout': async (_, context) => {
            await context.stdout.getWriter().abort(reason);
            return ok(undefined);
          },
        });

      await expect(
        tool.invoke('abort-stdout', [], invocationInput('abort-stdout'), undefined, {
          tag: 'anonymous',
        }),
      ).rejects.toBe(reason);
    });

    it('closes supplied stdin when the command does not declare it', async () => {
      const iteratorReturn = vi.fn().mockResolvedValue({ done: true, value: undefined });
      const stdin = {
        [Symbol.asyncIterator]: () => ({
          next: vi.fn(),
          return: iteratorReturn,
        }),
      };
      toolDefinition('no-streams')
        .body((body) => body.returns(z.void()))
        .implement({ 'no-streams': async () => ok(undefined) });

      await expect(
        tool.invoke('no-streams', [], invocationInput('no-streams'), stdin, { tag: 'anonymous' }),
      ).resolves.toEqual({ result: undefined, stdout: undefined });
      expect(iteratorReturn).toHaveBeenCalledOnce();
    });

    it('validates canonical input before handler and stdout setup', async () => {
      const handler = vi.fn(async () => ok(undefined));
      const OriginalWritableStream = globalThis.WritableStream;
      const writableStreamConstructor = vi.fn(
        (sink: UnderlyingSink<Uint8Array>) => new OriginalWritableStream(sink),
      );
      vi.stubGlobal('WritableStream', writableStreamConstructor);
      try {
        toolDefinition('stdout-validate')
          .body((body) =>
            body.positional('message', z.string()).stdout({ required: true }).returns(z.void()),
          )
          .implement({ 'stdout-validate': handler });
        const registered = ToolRegistry.get('stdout-validate');
        const commandNode = registered?.extended.commandByPath([]);
        if (!registered || !commandNode) throw new Error('stdout-validate tool was not registered');
        const inputModel = registered.extended.canonicalInputModel(commandNode);
        const invalidInput = typedSchemaValueToWit({
          graph: inputModel.codec.graph,
          value: v.record([v.bool(true)]),
        });

        await expect(
          tool.invoke('stdout-validate', [], invalidInput, undefined, { tag: 'anonymous' }),
        ).rejects.toMatchObject({ tag: 'invalid-input' });
        expect(handler).not.toHaveBeenCalled();
        expect(writableStreamConstructor).not.toHaveBeenCalled();
      } finally {
        vi.unstubAllGlobals();
      }
    });
  });

  it('does not convert undeclared handler exceptions into tool errors', async () => {
    const failure = new Error('handler failed');
    toolDefinition('traps')
      .body((body) => body.returns(z.void()))
      .implement({
        traps: async () => {
          throw failure;
        },
      });
    const registered = ToolRegistry.get('traps');
    const commandNode = registered?.extended.commandByPath([]);
    if (!registered || !commandNode) throw new Error('traps tool was not registered');
    const input = typedSchemaValueToWit(
      registered.extended.canonicalInputModel(commandNode).encodeTyped({}),
    );

    await expect(tool.invoke('traps', [], input, undefined, { tag: 'anonymous' })).rejects.toBe(
      failure,
    );
  });

  it('resolves a missing grafted child before enforcing its required stdin', async () => {
    const remote = toolDefinition('remote').body((body) =>
      body.stdin({ required: true }).returns(z.void()),
    );
    const git = toolDefinition('git').command('remote', remote);
    git.implement({});
    const registered = ToolRegistry.get('git');
    const commandNode = registered?.extended.commandByPath(['remote']);
    if (!registered || !commandNode) throw new Error('git subtree was not registered');
    const input = typedSchemaValueToWit(
      registered.extended.canonicalInputModel(commandNode).encodeTyped({}),
    );

    await expect(
      tool.invoke('git', ['remote'], input, undefined, { tag: 'anonymous' }),
    ).rejects.toEqual({
      tag: 'invalid-tool-name',
      val: 'remote',
    });
  });

  it('resolves an invalid command path before decoding malformed input', async () => {
    toolDefinition('echo')
      .body((body) => body.returns(z.void()))
      .implement({ echo: async () => ok(undefined) });
    const registered = ToolRegistry.get('echo');
    const commandNode = registered?.extended.commandByPath([]);
    if (!registered || !commandNode) throw new Error('echo tool was not registered');
    const validInput = typedSchemaValueToWit(
      registered.extended.canonicalInputModel(commandNode).encodeTyped({}),
    );
    const malformedInput = {
      ...validInput,
      value: {
        ...validInput.value,
        root: validInput.value.valueNodes.length,
      },
    };

    await expect(
      tool.invoke('echo', ['missing'], malformedInput, undefined, { tag: 'anonymous' }),
    ).rejects.toEqual({
      tag: 'invalid-command-path',
      val: ['missing'],
    });
  });
});
