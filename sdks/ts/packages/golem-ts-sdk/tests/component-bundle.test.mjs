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
  expect(discoverCapabilities(config)).toEqual(expected[name]);
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
        transform(code, id) {
          if (id.endsWith('.ts'))
            return {
              code: ts.transpileModule(code, { compilerOptions: config.options }).outputText,
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
function instantiate(code) {
  const module = { exports: {} };
  const host = {
    DatabaseSync: class {},
    getEnvironment: () => [['GOLEM_AGENT_ID', 'Counter()']],
    parseAgentId: () => [
      'Counter',
      { graph: { typeNodes: [], defs: [], root: 0 }, value: unit },
      undefined,
    ],
  };
  new Function('require', 'module', 'exports', code)(() => host, module, module.exports);
  return module.exports;
}

describe('static component exports', () => {
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
