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
import { ToolRegistry } from '../src/internal/registry/toolRegistry';

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
