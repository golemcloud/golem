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

import { beforeEach, describe, expect, it } from 'vitest';
import { z } from 'zod/v4';
import { command, ok, toolDefinition, type ToolImplementation } from '../src/fluent';
import { compileSchema } from '../src/fluent/schema/adapter';
import { ToolRegistry } from '../src/internal/registry/toolRegistry';
import { CanonicalInputModel } from '../src/internal/tool';

beforeEach(() => ToolRegistry.clearForTests());

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

  it('requires exact canonical field schemas when forwarding to a registered child', async () => {
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
