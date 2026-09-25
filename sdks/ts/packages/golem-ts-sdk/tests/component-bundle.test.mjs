import { describe, expect, it } from 'vitest';
import { rollup } from 'rollup';
import nodeResolve from '@rollup/plugin-node-resolve';
import ts from 'typescript';
import path from 'node:path';
import fs from 'node:fs';
import { z } from 'zod';
import { compileSchema } from '../src/schema/adapter';
import { typedSchemaValueToWit, schemaValueFromWit } from '../src/internal/schema-model';
import '../src/schema/zod';
import { componentPlugin, discoverCapabilities } from '../scripts/component.mjs';

const fixtures = path.resolve('tests/components');
const expected = {
  empty: { agents: false, tools: false, middleware: false },
  'tool-only': { agents: false, tools: true, middleware: false },
  'agent-only': { agents: true, tools: false, middleware: false },
  'exported-agent': { agents: true, tools: false, middleware: false },
  'agent-tool': { agents: true, tools: true, middleware: false },
  'agent-reflection': { agents: true, tools: false, middleware: false },
  'middleware-only': { agents: false, tools: false, middleware: true },
  mixed: { agents: true, tools: true, middleware: true },
};

function configuration(main) {
  return {
    fileNames: [main],
    options: {
      module: ts.ModuleKind.ESNext,
      moduleResolution: ts.ModuleResolutionKind.Bundler,
      target: ts.ScriptTarget.ES2022,
      skipLibCheck: true,
    },
  };
}

async function build(name) {
  const main = path.join(fixtures, `${name}.ts`);
  const config = configuration(main);
  if (expected[name]) expect(discoverCapabilities(config)).toEqual(expected[name]);
  let unminified;
  const bundle = await rollup({
    input: 'virtual:agent-main',
    external: (id) => /^(golem:|wasi:|node:|wasm-rquickjs:)/.test(id),
    onwarn(warning, warn) {
      if (warning.code !== 'CIRCULAR_DEPENDENCY') warn(warning);
    },
    plugins: [
      {
        name: 'inspect-dce',
        renderChunk(code) {
          unminified = code;
        },
      },
      componentPlugin(config, main),
      nodeResolve({ extensions: ['.ts', '.mjs', '.js'] }),
      {
        name: 'fixture-typescript',
        transform(_code, id) {
          if (id.endsWith('.ts'))
            return {
              // Like @rollup/plugin-typescript, emit from its TypeScript program
              // rather than preserving an earlier plugin's transformed source.
              code: ts.transpileModule(fs.readFileSync(id, 'utf8'), {
                compilerOptions: config.options,
              }).outputText,
              map: null,
            };
        },
      },
    ],
  });
  try {
    const { output } = await bundle.generate({ format: 'cjs', inlineDynamicImports: true });
    return { ...output[0], unminified };
  } finally {
    await bundle.close();
  }
}

const unit = { valueNodes: [{ tag: 'record-value', val: [] }], root: 0 };
function wireValue(schema, value) {
  const codec = compileSchema(schema);
  return typedSchemaValueToWit({ graph: codec.graph, value: codec.toValue(value) });
}
function instantiate(code, overrides = {}) {
  const module = { exports: {} };
  const host = {
    DatabaseSync: class {},
    getEnvironment: () => [['GOLEM_AGENT_ID', 'Counter()']],
    parseAgentId: () => [
      'Counter',
      { graph: { typeNodes: [], defs: [], root: 0 }, value: unit },
      undefined,
    ],
    ...overrides,
  };
  new Function('require', 'module', 'exports', code)(() => host, module, module.exports);
  return module.exports;
}

describe('static component exports', () => {
  it('executes compiler-emitted ordinary tool clients without retaining model validation', async () => {
    const output = await build('compiled-tool-client');
    const retained = Object.entries(output.modules)
      .filter(([, info]) => info.renderedLength > 0)
      .map(([id]) => id);
    for (const module of [
      '/schema-model/model.',
      '/schema-model/builder.',
      '/schema-model/validation.',
      '/internal/tool/model.',
      '/internal/tool/validation.',
    ])
      expect(retained.some((id) => id.includes(module))).toBe(false);
    expect(output.unminified).not.toContain('CanonicalInputModel');

    await instantiate(output.code);
    const client = globalThis.__golemCompiledToolClient;
    delete globalThis.__golemCompiledToolClient;
    expect(await client.asymmetric({ input: { count: 7, labels: [null, 'right'] } })).toEqual({
      label: 'left',
      values: [2, 9],
    });
    await expect(client.asymmetric({ input: { count: 1, labels: [] } })).rejects.toMatchObject({
      name: 'ToolCallError',
    });
    await expect(client.fail({})).rejects.toMatchObject({
      cause: { tag: 'tool', error: { name: 'broken', payload: { code: 41 } } },
    });
  }, 30000);

  it('shares one compiled agent definition between dispatch and clients, including typed stream items', async () => {
    const output = await build('compiled-agent');
    const retained = Object.entries(output.modules)
      .filter(([, info]) => info.renderedLength > 0)
      .map(([id]) => id)
      .join('\n');
    for (const module of [
      'schema-model/model.mjs',
      'schema-model/wit.mjs',
      'schema-model/validation.mjs',
    ])
      expect(retained).not.toContain(module);
    const calls = [];
    let closes = 0;
    let configReads = 0;
    let secretDrops = 0;
    let title;
    const runtime = await instantiate(output.code, {
      makeAgentId: () => 'Counter()',
      getConfigValue(path, graph) {
        expect(graph.typeNodes.length).toBeGreaterThan(0);
        if (path.join('.') === 'group.title')
          return title === undefined
            ? { valueNodes: [{ tag: 'option-value', val: undefined }], root: 0 }
            : {
                valueNodes: [
                  { tag: 'string-value', val: title },
                  { tag: 'option-value', val: 0 },
                ],
                root: 1,
              };
        expect(path).toEqual(['key']);
        configReads++;
        return {
          valueNodes: [
            {
              tag: 'secret-value',
              val: {
                [Symbol.dispose]() {
                  secretDrops++;
                },
              },
            },
          ],
          root: 0,
        };
      },
      reveal: () => ({
        valueNodes: [{ tag: 'string-value', val: `value${configReads}` }],
        root: 0,
      }),
      WasmRpc: class {
        asyncInvokeAndAwait(method, input) {
          calls.push({ method, input });
          return {
            metadata: {},
            future: {
              get: async () => ({ valueNodes: [{ tag: 'u32-value', val: 41 }], root: 0 }),
              cancel() {},
            },
          };
        }
      },
      SchemaValueStream: {
        wrap: async (iterable) => ({ iterable }),
        unwrap: async (wrapped) => wrapped.iterable,
      },
    });
    expect(runtime.guest.discoverAgentTypes()).toHaveLength(1);
    await runtime.guest.initialize('Counter', structuredClone(unit), { tag: 'anonymous' });
    expect(
      await runtime.guest.invoke('principal', structuredClone(unit), {
        tag: 'golem-user',
        val: { accountId: { uuid: { highBits: 17n, lowBits: 31n } } },
      }),
    ).toEqual({
      root: 5,
      valueNodes: [
        { tag: 'u64-value', val: 17n },
        { tag: 'u64-value', val: 31n },
        { tag: 'record-value', val: [0, 1] },
        { tag: 'record-value', val: [2] },
        { tag: 'record-value', val: [3] },
        { tag: 'variant-value', val: { case_: 2, payload: 4 } },
      ],
    });
    expect(configReads).toBe(0);
    expect(
      await runtime.guest.invoke('configured', structuredClone(unit), { tag: 'anonymous' }),
    ).toEqual({ valueNodes: [{ tag: 'string-value', val: 'absent:value1' }], root: 0 });
    title = 'present';
    expect(
      await runtime.guest.invoke('configured', structuredClone(unit), { tag: 'anonymous' }),
    ).toEqual({ valueNodes: [{ tag: 'string-value', val: 'present:value2' }], root: 0 });
    expect(secretDrops).toBe(2);
    const remote = await runtime.guest.invoke(
      'remote',
      {
        valueNodes: [
          { tag: 'u32-value', val: 17 },
          { tag: 'record-value', val: [0] },
        ],
        root: 1,
      },
      { tag: 'anonymous' },
    );
    expect(remote).toEqual({ valueNodes: [{ tag: 'u32-value', val: 41 }], root: 0 });
    expect(calls).toEqual([
      {
        method: 'scalar',
        input: {
          valueNodes: [
            { tag: 'u32-value', val: 17 },
            { tag: 'record-value', val: [0] },
          ],
          root: 1,
        },
      },
    ]);
    const item = {
      valueNodes: [
        { tag: 'u32-value', val: 13 },
        { tag: 'option-value', val: undefined },
        { tag: 'string-value', val: 'first' },
        { tag: 'option-value', val: 2 },
        { tag: 'list-value', val: [1, 3] },
        { tag: 'record-value', val: [0, 4] },
      ],
      root: 5,
    };
    const endpoint = () => ({
      iterable: {
        [Symbol.asyncIterator]() {
          let read = false;
          return {
            next: async () =>
              read
                ? { done: true }
                : ((read = true), { done: false, value: structuredClone(item) }),
            return: async () => {
              closes++;
              return { done: true };
            },
          };
        },
      },
    });
    const input = (suffix) => ({
      valueNodes: [
        { tag: 'stream-value', val: endpoint() },
        suffix,
        { tag: 'record-value', val: [0, 1] },
      ],
      root: 2,
    });
    const streamed = await runtime.guest.invoke(
      'stream',
      input({ tag: 'string-value', val: 'last' }),
      { tag: 'anonymous' },
    );
    const iterator = streamed.valueNodes[streamed.root].val.iterable[Symbol.asyncIterator]();
    expect(schemaValueFromWit((await iterator.next()).value)).toMatchObject({
      tag: 'record',
      fields: [
        { tag: 'u32', value: 16 },
        {
          tag: 'list',
          elements: [
            { tag: 'option', value: undefined },
            { tag: 'option', value: { value: 'first' } },
            { tag: 'option', value: { value: 'last' } },
          ],
        },
      ],
    });
    await iterator.return();
    expect(closes).toBe(1);
    await expect(
      runtime.guest.invoke('stream', input({ tag: 'bool-value', val: true }), { tag: 'anonymous' }),
    ).rejects.toMatchObject({ tag: 'invalid-input' });
    expect(closes).toBe(2);
  }, 30000);

  it('emits descriptors and concrete codecs for nested variants, recursive records and resources', async () => {
    const output = await build('compiled-tools');
    for (const symbol of [
      'CanonicalInputModel',
      'schemaValueConforms',
      'GraphEncoder',
      'SchemaValueReader',
      'compileSchema',
    ])
      expect(output.unminified).not.toContain(symbol);
    const retained = Object.entries(output.modules)
      .filter(([, info]) => info.renderedLength > 0)
      .map(([id]) => id);
    expect(retained.some((id) => id.includes('/schema-model/model.'))).toBe(false);
    expect(retained.some((id) => id.includes('/schema-model/validation.'))).toBe(false);
    const runtime = await instantiate(output.code);
    expect(runtime.tool.discoverTools()).toHaveLength(1);
    const graph = { typeNodes: [], defs: [], root: 999 };
    const invoke = (path, value) =>
      runtime.tool.invoke('compiled', path, { graph, value }, undefined, undefined, {
        tag: 'anonymous',
      });
    const choice = {
      valueNodes: [
        { tag: 'option-value', val: undefined },
        { tag: 'string-value', val: 'asymmetric' },
        { tag: 'option-value', val: 1 },
        { tag: 'list-value', val: [0, 2] },
        { tag: 'record-value', val: [3] },
      ],
      root: 4,
    };
    const result = await invoke(['n'], choice);
    expect(schemaValueFromWit(result.result.value)).toMatchObject({
      tag: 'variant',
      caseIndex: 1,
      payload: {
        tag: 'record',
        fields: [
          { tag: 'string', value: 'right' },
          {
            tag: 'list',
            elements: [
              { tag: 'option', value: undefined },
              { tag: 'option', value: { tag: 'string', value: 'asymmetric' } },
            ],
          },
        ],
      },
    });
    const wrongTag = structuredClone(choice);
    wrongTag.valueNodes[1] = { tag: 'bool-value', val: true };
    await expect(invoke(['nested'], wrongTag)).rejects.toMatchObject({ tag: 'invalid-input' });
    const alias = structuredClone(choice);
    alias.valueNodes[3].val = [0, 0];
    await expect(invoke(['nested'], alias)).rejects.toMatchObject({ tag: 'invalid-input' });
    await expect(
      invoke(['n', 'fail'], {
        valueNodes: [
          { tag: 'u32-value', val: 17 },
          { tag: 'record-value', val: [0] },
        ],
        root: 1,
      }),
    ).rejects.toMatchObject({
      tag: 'custom-error',
      val: {
        name: 'broken',
        payload: {
          value: {
            valueNodes: [
              { tag: 'u32-value', val: 22 },
              { tag: 'record-value', val: [0] },
            ],
            root: 1,
          },
        },
      },
    });
    const tree = {
      valueNodes: [
        { tag: 'string-value', val: 'leaf' },
        { tag: 'list-value', val: [] },
        { tag: 'record-value', val: [0, 1] },
        { tag: 'string-value', val: 'root' },
        { tag: 'list-value', val: [2] },
        { tag: 'record-value', val: [3, 4] },
        { tag: 'record-value', val: [5] },
      ],
      root: 6,
    };
    expect(schemaValueFromWit((await invoke(['tree'], tree)).result.value)).toMatchObject({
      tag: 'record',
      fields: [
        { value: 'root' },
        {
          tag: 'list',
          elements: [{ tag: 'record', fields: [{ value: 'leaf' }, { tag: 'list', elements: [] }] }],
        },
      ],
    });
    const concrete = await invoke(['concrete'], {
      valueNodes: [
        { tag: 'u16-value', val: 7 },
        { tag: 'u16-value', val: 8 },
        { tag: 'list-value', val: [0, 1] },
        { tag: 'record-value', val: [2] },
      ],
      root: 3,
    });
    expect(concrete.result.value).toEqual({
      valueNodes: [
        {
          tag: 'binary-value',
          val: { bytes: new Uint8Array([7, 2]), mimeType: 'application/test' },
        },
        { tag: 'variant-value', val: { case_: 0, payload: 0 } },
      ],
      root: 1,
    });
    await expect(invoke(['restricted-binary'], unit)).rejects.toMatchObject({
      tag: 'invalid-result',
    });
    expect((await invoke(['valibot'], unit)).result.value).toEqual({
      valueNodes: [
        { tag: 'string-value', val: 'label' },
        { tag: 'string-value', val: 'four' },
        { tag: 'record-value', val: [0, 1] },
        { tag: 'variant-value', val: { case_: 1, payload: 2 } },
      ],
      root: 3,
    });
    let drops = 0;
    const raw = {
      [Symbol.dispose]() {
        drops++;
      },
    };
    const resourceInput = (count) => ({
      valueNodes: [
        { tag: 'secret-value', val: raw },
        { tag: 'u32-value', val: count },
        { tag: 'record-value', val: [0, 1] },
        { tag: 'record-value', val: [2] },
      ],
      root: 3,
    });
    await expect(invoke(['secret'], resourceInput(-1))).rejects.toMatchObject({
      tag: 'invalid-input',
    });
    expect(drops).toBe(1);
    const fresh = {
      [Symbol.dispose]() {
        drops++;
      },
    };
    const valid = resourceInput(23);
    valid.valueNodes[0].val = fresh;
    expect((await invoke(['secret'], valid)).result.value.valueNodes[0].val).toBe(fresh);
    expect(valid.valueNodes[0].val).toBeUndefined();
    expect(drops).toBe(1);
  }, 30000);

  it('resolves application initialization before exposing synchronous WIT exports', async () => {
    const bundle = await rollup({
      input: path.resolve('dist/wrapper.mjs'),
      plugins: [
        {
          name: 'injected-user',
          resolveId(id) {
            if (id === 'user') return '\0user';
          },
          load(id) {
            if (id === '\0user')
              return `import * as exports from ${JSON.stringify(path.resolve('dist/runtime/emptyGuest.mjs'))}; export default Promise.resolve(exports);`;
          },
        },
      ],
    });
    try {
      const { output } = await bundle.generate({ format: 'es', inlineDynamicImports: true });
      const wrapper = await import(
        `data:text/javascript;base64,${Buffer.from(output[0].code).toString('base64')}`
      );
      expect(wrapper.golemAgent200Guest).toBe(wrapper.guest);
      expect(wrapper.golemTool010Guest).toBe(wrapper.tool);
      expect(wrapper.golemTool010ToolMiddlewareGuest).toBe(wrapper.toolMiddlewareGuest);
      expect(wrapper.guest.discoverAgentTypes()).toEqual([]);
      expect(wrapper.tool.discoverTools()).toEqual([]);
      expect(wrapper.toolMiddlewareGuest.discoverToolMiddlewares()).toEqual([]);
    } finally {
      await bundle.close();
    }
  });

  for (const [name, capabilities] of Object.entries(expected)) {
    it(`${name}: discovers capabilities, eliminates unused runtimes and preserves mandatory exports`, async () => {
      const output = await build(name);
      const retained = Object.entries(output.modules)
        .filter(([, info]) => info.renderedLength > 0)
        .map(([id]) => id)
        .join('\n');
      for (const [capability, modules] of [
        ['agents', ['agentTypeRegistry.mjs', 'agentInitiatorRegistry.mjs', 'multipart.mjs']],
        ['middleware', ['toolMiddlewareRegistry.mjs', 'middlewareRuntime.mjs']],
      ]) {
        if (!capabilities[capability])
          for (const module of modules) expect(retained).not.toContain(module);
      }
      if (!capabilities.tools) expect(output.unminified).not.toContain('class ToolRegistryImpl');
      if (!capabilities.middleware && name !== 'agent-reflection') {
        for (const module of [
          'schema-model/model.mjs',
          'schema-model/wit.mjs',
          'schema-model/validation.mjs',
        ])
          expect(retained).not.toContain(module);
      }
      if (name === 'agent-reflection') expect(retained).toContain('schema-model/wit.mjs');
      if (!capabilities.agents) {
        expect(output.unminified).not.toContain('serializePrincipal');
        expect(output.unminified).not.toContain('initializedAgent');
      }
      const exports = await instantiate(output.code);
      expect(Object.keys(exports).sort()).toEqual([
        'guest',
        'loadSnapshot',
        'saveSnapshot',
        'tool',
        'toolMiddlewareGuest',
      ]);
      expect(exports.guest.discoverAgentTypes().map((a) => a.typeName)).toEqual(
        capabilities.agents ? ['Counter'] : [],
      );
      expect(exports.tool.discoverTools().map((t) => t.commands.nodes[0].name)).toEqual(
        capabilities.tools ? ['greet'] : [],
      );
      expect(exports.toolMiddlewareGuest.discoverToolMiddlewares().map((m) => m.name)).toEqual(
        capabilities.middleware ? ['greeting-policy'] : [],
      );
      if (!capabilities.agents) {
        await expect(
          exports.guest.initialize('missing', unit, { tag: 'anonymous' }),
        ).rejects.toEqual({ tag: 'invalid-type', val: 'missing' });
        await expect(
          exports.loadSnapshot.load({ payload: new Uint8Array(), mimeType: 'application/json' }),
        ).rejects.toBe('This component does not define agents');
      } else {
        await exports.guest.initialize('Counter', unit, { tag: 'anonymous' });
        const result = await exports.guest.invoke(
          'add',
          wireValue(z.object({ amount: z.number() }), { amount: 11 }).value,
          { tag: 'anonymous' },
        );
        expect(compileSchema(z.number()).fromValue(schemaValueFromWit(result))).toBe(18);
        const snapshot = await exports.saveSnapshot.save();
        expect(JSON.parse(new TextDecoder().decode(snapshot.payload))).toEqual({
          version: 1,
          principal: { tag: 'anonymous' },
          state: { count: 18 },
        });
        const restored = await instantiate(output.code);
        await restored.loadSnapshot.load(snapshot);
        expect(await restored.saveSnapshot.save()).toEqual(snapshot);
      }
      const input = wireValue(z.object({ name: z.string() }), { name: 'Ada' });
      if (capabilities.tools) {
        const result = await exports.tool.invoke('greet', [], input, undefined, undefined, {
          tag: 'anonymous',
        });
        expect(compileSchema(z.string()).fromValue(schemaValueFromWit(result.result.value))).toBe(
          'Hello, Ada!',
        );
        const poisonGraphInput = {
          graph: { typeNodes: [], defs: [], root: 99 },
          value: input.value,
        };
        await expect(
          exports.tool.invoke('greet', [], poisonGraphInput, undefined, undefined, {
            tag: 'anonymous',
          }),
        ).resolves.toHaveProperty('result.value');
        await expect(
          exports.tool.invoke(
            'greet',
            [],
            { graph: poisonGraphInput.graph, value: unit },
            undefined,
            undefined,
            { tag: 'anonymous' },
          ),
        ).rejects.toMatchObject({ tag: 'invalid-input' });
      } else {
        await expect(
          exports.tool.invoke('greet', [], input, undefined, undefined, { tag: 'anonymous' }),
        ).rejects.toEqual({ tag: 'invalid-tool-name', val: 'greet' });
        let closed = false;
        const stdin = {
          [Symbol.asyncIterator]() {
            return {
              next: async () => ({ done: true }),
              async return() {
                closed = true;
                throw new Error('Cleanup failure');
              },
            };
          },
        };
        await expect(
          exports.tool.invoke('greet', [], input, stdin, undefined, { tag: 'anonymous' }),
        ).rejects.toEqual({ tag: 'invalid-tool-name', val: 'greet' });
        expect(closed).toBe(true);
      }
      if (capabilities.middleware) {
        const middleware = exports.toolMiddlewareGuest.getToolMiddleware('greeting-policy');
        const result = await exports.toolMiddlewareGuest.invokeToolMiddleware(
          'greeting-policy',
          'greet',
          middleware.scope.val.presented,
          wireValue(z.object({}), {}),
          [],
          input,
          undefined,
          undefined,
          { tag: 'anonymous' },
          {
            invoke() {
              throw new Error('The short-circuit middleware must not invoke the underlying tool');
            },
          },
        );
        expect(compileSchema(z.string()).fromValue(schemaValueFromWit(result.result.value))).toBe(
          'Welcome, Ada!',
        );
      }
      console.log(`${name}: ${Buffer.byteLength(output.code)} JS bytes`);
    }, 30000);
  }

  it('resolves aliases, namespace and destructured registrations, with conservative computed access', () => {
    const dir = fs.mkdtempSync(path.join(fixtures, '.capabilities-'));
    try {
      for (const [source, expectedCapabilities] of [
        [
          'import { defineAgent as agent } from "@golemcloud/golem-ts-sdk"; agent({name:"A", id:{}, methods:{}});',
          { agents: true, tools: false, middleware: false },
        ],
        [
          'import * as sdk from "@golemcloud/golem-ts-sdk"; sdk.toolDefinition("x")["implement"]({});',
          { agents: false, tools: true, middleware: false },
        ],
        [
          'import { toolDefinition } from "@golemcloud/golem-ts-sdk"; const { implement: register } = toolDefinition("x"); register({});',
          { agents: false, tools: true, middleware: false },
        ],
        [
          'import { toolDefinition } from "@golemcloud/golem-ts-sdk"; const { middleware: register } = toolDefinition("x"); register({});',
          { agents: false, tools: false, middleware: true },
        ],
        [
          'import { universalToolMiddleware as policy } from "@golemcloud/golem-ts-sdk"; policy({});',
          { agents: false, tools: false, middleware: true },
        ],
        [
          'import type { AgentDefinition } from "@golemcloud/golem-ts-sdk"; export type Definition = AgentDefinition;',
          { agents: false, tools: false, middleware: false },
        ],
        [
          'import { toolDefinition } from "@golemcloud/golem-ts-sdk"; const key = Math.random() ? "implement" : "middleware"; toolDefinition("x")[key]({});',
          { agents: true, tools: true, middleware: true },
        ],
      ]) {
        const main = path.join(dir, 'main.ts');
        fs.writeFileSync(main, source);
        expect(discoverCapabilities(configuration(main)), source).toEqual(expectedCapabilities);
      }
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  }, 30000);
});
