import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import { z as z3 } from 'zod3';
import * as valibot from 'valibot';
import { type } from 'arktype';
import { getConfigValue } from 'golem:agent/host@2.0.0';
import { buildConfigAccessor, compileConfig, compileConfigTree } from '../src/config';
import { s } from '../src/schema/markers';
import { SchemaRef } from '../src/schema/ref';
import { defineAgent } from '../src/defineAgent';
import { method } from '../src/method';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentClassName } from '../src/agentClassName';
import {
  schemaValueToWit,
  schemaValueFromWit,
  schemaGraphFromWit,
} from '../src/internal/schema-model';
import '../src/schema/valibot';
import '../src/schema/arktype';

const corpus = JSON.parse(
  readFileSync(
    new URL('../../../../../test-data/config-conformance/v1.json', import.meta.url),
    'utf8',
  ),
);

const choice = s.union([
  { tag: 'Text', schema: z.string(), discriminator: { tag: 'prefix', val: 'selected:' } },
  { tag: 'Other', schema: z.string(), discriminator: { tag: 'prefix', val: 'other:' } },
]);

describe('static config', () => {
  it('exports the independent reference defaults, names, widths, tags and tuple positions', () => {
    const config = {
      settings: s.default(z.object({ label: z.string(), count: s.u32() }), {
        label: 'code-default',
        count: 31,
      }),
      choice: s.default(choice, 'selected:value'),
      slots: s.default(s.tuple([s.option(s.s32()), s.option(s.s32()), z.string()]), [
        null,
        7,
        'retained',
      ]),
      omitted: z.string().optional(),
    };
    defineAgent({ name: 'StaticConfigMetadata', id: {}, methods: {}, config });
    const agent = AgentTypeRegistry.get(new AgentClassName('StaticConfigMetadata'))!;
    const expected = corpus.schemas.reference;
    expect(
      agent.config.map((entry) => ({
        path: entry.path,
        source: entry.source,
        default:
          entry.defaultValue &&
          new SchemaRef(schemaGraphFromWit({ ...agent.schema, root: entry.valueType })).unpackJson(
            schemaValueFromWit(entry.defaultValue),
          ),
      })),
    ).toEqual(
      expected.declarations.map((entry: any) => ({
        path: entry.path,
        source: 'local',
        default: entry.default_value && JSON.parse(entry.default_value),
      })),
    );
    const declarations = compileConfig(config);
    const settings = declarations[0].graph.root.body;
    expect(settings.tag).toBe('record');
    if (settings.tag !== 'record') throw new Error('expected record');
    const expectedSettings = expected.graph.defs.find((def: any) => def.id === 'Settings').body
      .value.fields;
    expect(settings.fields.map((field) => [field.name, field.body.body.tag])).toEqual(
      expectedSettings.map((field: any) => [field.name, field.body.kind]),
    );
    const union = declarations[1].graph.root.body;
    if (union.tag !== 'union') throw new Error('expected union');
    expect(
      union.branches.map((branch) => ({
        tag: branch.tag,
        type: branch.body.body.tag,
        discriminator: branch.discriminator,
      })),
    ).toEqual(
      expected.graph.defs
        .find((def: any) => def.id === 'Choice')
        .body.value.spec.branches.map((branch: any) => ({
          tag: branch.tag,
          type: branch.body.kind,
          discriminator: { tag: branch.discriminator.rule, val: branch.discriminator.value.prefix },
        })),
    );
    const tuple = declarations[2].graph.root.body;
    if (tuple.tag !== 'tuple') throw new Error('expected tuple');
    expect(
      tuple.elements.map((item) =>
        item.body.tag === 'option' ? ['option', item.body.element.body.tag] : [item.body.tag],
      ),
    ).toEqual(
      expected.declarations[2].value_type.value.elements.map((item: any) =>
        item.kind === 'option' ? ['option', item.value.inner.kind] : [item.kind],
      ),
    );
    const slots = new SchemaRef(declarations[2].graph).packJson([null, 7, 'retained']);
    expect(declarations[2].codec.fromValue(slots)).toEqual([undefined, 7, 'retained']);
  });

  it.each([
    ['Zod', () => z.object({ max_chars: z.number(), labels: z.array(z.string().optional()) })],
    [
      'Valibot',
      () =>
        valibot.object({
          max_chars: valibot.number(),
          labels: valibot.array(valibot.optional(valibot.string())),
        }),
    ],
    ['ArkType', () => type({ max_chars: 'number', labels: '(string | undefined)[]' })],
  ] as const)(
    '%s keeps static records whole and projects names without recasing values',
    (_, schema) => {
      const [declaration] = compileConfig({
        MaxChars: s.default(schema(), {
          maxChars: 13,
          labels: ['labels.Support_default', null, ''],
        }),
      });
      expect(declaration.path).toEqual(corpus.schemas.canonicalName.declarations[0].path);
      expect(new SchemaRef(declaration.graph).unpackJson(declaration.defaultValue!)).toEqual({
        maxChars: 13,
        labels: ['labels.Support_default', null, ''],
      });
      const value = declaration.defaultValue!;
      expect(declaration.codec.fromValue(value)).toEqual({
        max_chars: 13,
        labels: ['labels.Support_default', undefined, ''],
      });
    },
  );

  it.each([
    [
      'Zod',
      () => {
        const Node: z.ZodType = z.lazy(() =>
          z.object({ label: z.string(), children: z.array(Node) }),
        );
        return Node;
      },
    ],
    [
      'Valibot',
      () => {
        const Node: any = valibot.lazy(() =>
          valibot.object({ label: valibot.string(), children: valibot.array(Node) }),
        );
        return Node;
      },
    ],
    ['ArkType', () => type.scope({ node: { label: 'string', children: 'node[]' } }).export().node],
  ] as const)('%s retains recursive graphs and values', (_, schema) => {
    const data = { label: 'root', children: [{ label: 'child', children: [] }] };
    const [declaration] = compileConfig({ node: s.default(schema(), data) });
    expect(declaration.graph.defs.size).toBeGreaterThan(0);
    expect(declaration.codec.fromValue(new SchemaRef(declaration.graph).packJson(data))).toEqual(
      data,
    );
  });

  it.each([
    [
      'Zod',
      () =>
        z.discriminatedUnion('delivery_kind', [
          z.object({ delivery_kind: z.literal('mail'), address: z.string() }),
          z.object({ delivery_kind: z.literal('queue'), queue_name: z.string() }),
        ]),
    ],
    [
      'Valibot',
      () =>
        valibot.variant('delivery_kind', [
          valibot.object({ delivery_kind: valibot.literal('mail'), address: valibot.string() }),
          valibot.object({ delivery_kind: valibot.literal('queue'), queue_name: valibot.string() }),
        ]),
    ],
    [
      'ArkType',
      () =>
        type({ delivery_kind: "'mail'", address: 'string' }).or({
          delivery_kind: "'queue'",
          queue_name: 'string',
        }),
    ],
  ] as const)('%s retains record union discriminators and branch tags', (_, schema) => {
    const [declaration] = compileConfig({
      delivery: s.default(schema(), { deliveryKind: 'queue', queueName: 'work' }),
    });
    const body = declaration.graph.root.body;
    if (body.tag !== 'union') throw new Error('expected union');
    expect(new Set(body.branches.map((branch) => branch.tag))).toEqual(new Set(['mail', 'queue']));
    expect(body.branches.map((branch) => branch.discriminator)).toEqual(
      body.branches.map((branch) => ({
        tag: 'field-equals',
        val: { fieldName: 'deliveryKind', literal: branch.tag },
      })),
    );
    expect(
      declaration.codec.fromValue(
        new SchemaRef(declaration.graph).packJson({ deliveryKind: 'queue', queueName: 'work' }),
      ),
    ).toEqual({ delivery_kind: 'queue', queue_name: 'work' });
  });

  it('can reuse a recursive schema in config and methods without merging different field names', () => {
    const Node: z.ZodType = z.lazy(() =>
      z.object({ display_name: z.string(), children: z.array(Node) }),
    );
    defineAgent({
      name: 'ConfigAndMethodGraph',
      id: {},
      config: { node: Node },
      methods: { echo: method({ input: { value: Node }, returns: Node }) },
    });
    expect(AgentTypeRegistry.get(new AgentClassName('ConfigAndMethodGraph'))).toBeDefined();
  });

  it('extracts data-only Valibot and ArkType defaults, including false, empty and null', () => {
    const declarations = compileConfig({
      disabled: valibot.optional(valibot.boolean(), false),
      text: valibot.optional(valibot.string(), ''),
      absent: valibot.nullable(valibot.string(), null),
      object: type({ count: 'number = 31', label: 'string = "code-default"' }),
    });
    expect(
      declarations.map((declaration) =>
        new SchemaRef(declaration.graph).unpackJson(declaration.defaultValue!),
      ),
    ).toEqual([false, '', null, { count: 31, label: 'code-default' }]);
    expect(compileConfig({ omitted: z.string().optional() })[0].defaultValue).toBeUndefined();
  });

  it('projects native record defaults to canonical names, including nested union fields', () => {
    const [declaration] = compileConfig({
      settings: valibot.optional(
        valibot.object({
          max_chars: valibot.number(),
          delivery: valibot.variant('delivery_kind', [
            valibot.object({
              delivery_kind: valibot.literal('mail'),
              mail_address: valibot.string(),
            }),
            valibot.object({
              delivery_kind: valibot.literal('queue'),
              queue_name: valibot.string(),
            }),
          ]),
        }),
        { max_chars: 13, delivery: { delivery_kind: 'queue', queue_name: 'work' } },
      ),
    });
    expect(new SchemaRef(declaration.graph).unpackJson(declaration.defaultValue!)).toEqual({
      maxChars: 13,
      delivery: { deliveryKind: 'queue', queueName: 'work' },
    });
    expect(declaration.codec.fromValue(declaration.defaultValue!)).toEqual({
      max_chars: 13,
      delivery: { delivery_kind: 'queue', queue_name: 'work' },
    });
    const [ark] = compileConfig({
      settings: type({ limits: { max_chars: 'number = 13' } }),
    });
    expect(new SchemaRef(ark.graph).unpackJson(ark.defaultValue!)).toEqual({
      limits: { maxChars: 13 },
    });
  });

  it('rejects native defaults that lose or rewrite source data', () => {
    for (const value of [{ kind: 'wrong' }, {}, { kind: 'fixed', extra: 7 }]) {
      expect(() =>
        compileConfig({
          value: valibot.optional(valibot.object({ kind: valibot.literal('fixed') }), value as any),
        }),
      ).toThrow(/default/);
    }
  });

  it('canonicalizes acronym paths and rejects collisions', () => {
    expect(
      compileConfig({ API_KEY: z.string(), HTTPServer: z.string(), baseURL: z.string() }).map(
        (entry) => entry.path,
      ),
    ).toEqual([['apiKey'], ['httpServer'], ['baseUrl']]);
    expect(() => compileConfig({ API_KEY: z.string(), apiKey: z.string() })).toThrow(
      /Duplicate canonical/,
    );
  });

  it('validates selected union branches and container constraints with SDK markers', () => {
    const schemas = [
      z
        .discriminatedUnion('kind', [
          z.object({ kind: z.literal('a'), count: s.u32() }),
          z.object({ kind: z.literal('b'), label: z.string() }),
        ])
        .refine((value) => value.kind !== 'a' || value.count > 10),
      z3
        .discriminatedUnion('kind', [
          z3.object({ kind: z3.literal('a'), count: s.u32() as any }),
          z3.object({ kind: z3.literal('b'), label: z3.string() }),
        ])
        .refine((value) => value.kind !== 'a' || value.count > 10),
      valibot.pipe(
        valibot.variant('kind', [
          valibot.object({ kind: valibot.literal('a'), count: s.u32() as any }),
          valibot.object({ kind: valibot.literal('b'), label: valibot.string() }),
        ]),
        valibot.check((value) => value.kind !== 'a' || value.count > 10),
      ),
      ...[
        z.object({ kind: z.literal('a'), count: s.u32() }).refine((value) => value.count > 10),
        z3
          .object({ kind: z3.literal('a'), count: s.u32() as any })
          .refine((value) => value.count > 10),
        valibot.pipe(
          valibot.object({ kind: valibot.literal('a'), count: s.u32() as any }),
          valibot.check((value) => value.count > 10),
        ),
      ].map((schema) =>
        s.union([
          {
            tag: 'a',
            schema,
            discriminator: { tag: 'field-equals', val: { fieldName: 'kind', literal: 'a' } },
          },
          {
            tag: 'b',
            schema: z.object({ kind: z.literal('b'), label: z.string() }),
            discriminator: { tag: 'field-equals', val: { fieldName: 'kind', literal: 'b' } },
          },
        ]),
      ),
    ];
    for (const schema of schemas) {
      for (const value of [
        { kind: 'a', count: 11 },
        { kind: 'b', label: 'other' },
      ]) {
        const [entry] = compileConfig({ choice: s.default(schema, value) });
        expect(new SchemaRef(entry.graph).unpackJson(entry.defaultValue!)).toEqual(value);
      }
      for (const count of [10, -1]) {
        expect(() => compileConfig({ choice: s.default(schema, { kind: 'a', count }) })).toThrow();
      }
    }
  });

  it('rejects nullable leaves below optional managed groups instead of emitting nested options', () => {
    for (const value of [z.string().nullable(), s.default(z.string().nullable(), null)]) {
      expect(() =>
        compileConfig({
          service: z.object({ value, key: s.secret(z.string()) }).optional(),
        }),
      ).toThrow(/Nullable static config.*service.value.*optional group/);
    }
    const [entry] = compileConfig({ value: s.default(z.string().nullable(), null) });
    expect(entry.defaultValue).toEqual({ tag: 'option', value: undefined });
    expect(entry.codec.fromValue(entry.defaultValue!)).toBeNull();
  });

  it('rejects transforming scalar checks before validating static defaults', () => {
    let overwriteCalls = 0;
    const overwrite = z.string().overwrite((value) => {
      overwriteCalls++;
      return value.trim();
    });
    for (const schema of [
      z3
        .string()
        .trim()
        .refine((value) => value === 'x'),
      z3.string().toLowerCase(),
      z3.string().toUpperCase(),
      z
        .string()
        .trim()
        .refine((value) => value === 'x'),
      overwrite,
    ]) {
      expect(() => compileConfig({ value: s.default(schema, ' x ') })).toThrow(
        /transformations|overwrites/,
      );
      expect(() => compileConfig({ value: schema })).toThrow(/transformations|overwrites/);
    }
    expect(overwriteCalls).toBe(0);
  });

  it('allows refinement and identity wrappers without executing transformations', () => {
    const calls = vi.fn((value: string) => value);
    expect(() =>
      compileConfig({
        value: s.default(
          valibot.pipe(
            valibot.pipe(valibot.string(), valibot.transform(calls)),
            valibot.minLength(1),
          ),
          'valid',
        ),
      }),
    ).toThrow(/transformation/);
    expect(calls).not.toHaveBeenCalled();
    for (const schema of [
      z3.string().refine((value) => value.length > 2),
      valibot.pipe(valibot.string(), valibot.brand('Name'), valibot.readonly()),
    ]) {
      expect(compileConfig({ value: schema })).toHaveLength(1);
      expect(compileConfig({ value: s.default(schema, 'valid') })).toHaveLength(1);
    }
    expect(() =>
      compileConfig({
        value: s.default(
          z3.string().refine((value) => value.length > 2),
          'x',
        ),
      }),
    ).toThrow(/Invalid static config default/);
  });

  it('never calls opaque factories or getters, and rejects unsupported defaults clearly', () => {
    const callback = vi.fn(() => 'unsafe');
    const schemas = [
      z.string().default(callback),
      z3.string().default(callback),
      valibot.optional(valibot.string(), callback),
      type({ value: type('string').default(callback) }),
    ];
    // ArkType itself probes factories while constructing its native schema.
    callback.mockClear();
    for (const schema of schemas) {
      expect(() => compileConfig({ value: schema as any })).toThrow(
        /opaque native config default|s.default/,
      );
    }
    expect(callback).not.toHaveBeenCalled();
    expect(() =>
      s.default(z.string(), {
        get value() {
          callback();
          return 'unsafe';
        },
      }),
    ).toThrow(/accessors/);
    expect(callback).not.toHaveBeenCalled();
    expect(() => compileConfig({ value: s.default(z.string().min(3), 'x') })).toThrow(
      /Invalid static config default/,
    );
    expect(() => compileConfig({ value: s.default(s.u32(), -1) })).toThrow();
    expect(() => compileConfig({ value: valibot.fallback(valibot.string(), 'fallback') })).toThrow(
      /fallback/,
    );
    expect(() =>
      compileConfig({ value: valibot.pipe(valibot.string(), valibot.transform(callback)) }),
    ).toThrow(/transformation/);
    expect(callback).not.toHaveBeenCalled();
    expect(() => compileConfig({ value: s.default(s.secret(z.string()), 'secret') })).toThrow(
      /Secret config/,
    );
    expect(() => compileConfig({ value: z.array(s.secret(z.string())) })).toThrow(/capabilities/);
    expect(() => compileConfig({ value: z.object({ $reserved: z.string() }) })).toThrow(
      /leading '\$'/,
    );
    expect(() => compileConfig({ max_chars: z.string(), maxChars: z.string() })).toThrow(
      /Duplicate canonical/,
    );
    expect(() =>
      compileConfig({ group: z.object({ max_chars: z.string(), maxChars: s.secret(z.string()) }) }),
    ).toThrow(/Duplicate canonical/);
    expect(() => compileConfig({ values: type({ count: 'number = 31' }).array() })).toThrow(
      /Nested config defaults/,
    );
    expect(() =>
      compileConfig({ values: type({ 'limits?': { max_chars: 'number = 13' } }) }),
    ).toThrow(/Nested config defaults/);
    expect(() =>
      compileConfig({
        value: valibot.object({
          count: valibot.optional(valibot.number(), 31),
          label: valibot.string(),
        }),
      }),
    ).toThrow(/partial nested config default/);
  });

  it('validates container constraints on static defaults', () => {
    const bounded = [
      z.array(z.number()).min(1).max(2),
      z
        .array(s.u32() as any)
        .min(1)
        .max(2),
      z3
        .array(s.u32() as any)
        .min(1)
        .max(2),
      valibot.pipe(valibot.array(s.u32() as any), valibot.minLength(1), valibot.maxLength(2)),
      type('number[] >= 1').atMostLength(2),
    ];
    for (const schema of bounded) {
      for (const invalid of [[], [2, 7, 9]]) {
        expect(() => compileConfig({ values: s.default(schema, invalid) })).toThrow(
          /Invalid static config default/,
        );
      }
      const [valid] = compileConfig({ values: s.default(schema, [2, 7]) });
      expect(new SchemaRef(valid.graph).unpackJson(valid.defaultValue!)).toEqual([2, 7]);
    }
    const ordered = z
      .object({ lower: s.u32(), upper: s.u32() })
      .refine(({ lower, upper }) => lower < upper);
    expect(() => compileConfig({ range: s.default(ordered, { lower: 9, upper: 2 }) })).toThrow(
      /Invalid static config default/,
    );
    const [valid] = compileConfig({ range: s.default(ordered, { lower: 2, upper: 9 }) });
    expect(new SchemaRef(valid.graph).unpackJson(valid.defaultValue!)).toEqual({
      lower: 2,
      upper: 9,
    });
    const readonlyOrdered = z
      .object({ lower: s.u32(), upper: s.u32() })
      .readonly()
      .refine(({ lower, upper }) => lower < upper);
    expect(() =>
      compileConfig({ range: s.default(readonlyOrdered, { lower: 9, upper: 2 }) }),
    ).toThrow(/Invalid static config default/);
    const [readonlyValid] = compileConfig({
      range: s.default(readonlyOrdered, { lower: 2, upper: 9 }),
    });
    expect(new SchemaRef(readonlyValid.graph).unpackJson(readonlyValid.defaultValue!)).toEqual({
      lower: 2,
      upper: 9,
    });
    const piped = valibot.pipe(
      valibot.pipe(
        valibot.object({ lower: s.u32(), upper: s.u32() }),
        valibot.check(({ lower, upper }) => lower < upper),
      ),
      valibot.check(({ upper }) => upper < 10),
    );
    for (const invalid of [
      { lower: 9, upper: 2 },
      { lower: 2, upper: 10 },
    ]) {
      expect(() => compileConfig({ range: s.default(piped, invalid) })).toThrow(
        /Invalid static config default/,
      );
    }
    const [pipedValid] = compileConfig({ range: s.default(piped, { lower: 2, upper: 9 }) });
    expect(new SchemaRef(pipedValid.graph).unpackJson(pipedValid.defaultValue!)).toEqual({
      lower: 2,
      upper: 9,
    });
    expect(() =>
      compileConfig({
        values: s.default(
          z.array(z.number()).overwrite(() => []),
          [2, 7],
        ),
      }),
    ).toThrow(/overwrites/);
    expect(() =>
      compileConfig({
        values: s.default(
          z.array(z.number()).refine(async () => true),
          [2, 7],
        ),
      }),
    ).toThrow(/asynchronous validation/);
  });

  it('exports independent static child defaults beside managed fields', () => {
    const [timeout, apiKey] = compileConfig({
      service: z.object({
        timeout: s.default(z.number(), 30),
        apiKey: s.secret(z.string()),
      }),
    });
    expect(timeout.path).toEqual(['service', 'timeout']);
    expect(new SchemaRef(timeout.graph).unpackJson(timeout.defaultValue!)).toBe(30);
    expect(apiKey.path).toEqual(['service', 'apiKey']);
    expect(apiKey.source).toBe('secret');
    expect(apiKey.defaultValue).toBeUndefined();
    expect(() =>
      compileConfig({
        service: s.default(z.object({ apiKey: s.secret(z.string()) }), { apiKey: 'secret' }),
      }),
    ).toThrow(/managed fields/);
  });

  it('validates defaults below optional managed groups', () => {
    for (const value of ['x', 'valid']) {
      const compile = () =>
        compileConfig({
          service: z
            .object({
              label: s.default(z.string().min(3), value),
              apiKey: s.secret(z.string()),
            })
            .optional(),
        });
      if (value === 'x') {
        expect(compile).toThrow(/Invalid static config default/);
      } else {
        const [label] = compile();
        expect(new SchemaRef(label.graph).unpackJson(label.defaultValue!)).toBe('valid');
      }
    }
  });

  it('reads the host value at the canonical path with no SDK-local default fallback', () => {
    const tree = compileConfigTree({ max_chars: s.default(z.number().optional(), 31) });
    const [declaration] = compileConfig({ max_chars: s.default(z.number().optional(), 31) });
    vi.mocked(getConfigValue).mockReturnValue(schemaValueToWit(declaration.codec.toValue(59)));
    const config = buildConfigAccessor(tree);
    expect(config.max_chars).toBe(59);
    expect(vi.mocked(getConfigValue).mock.lastCall?.[0]).toEqual(['maxChars']);
    vi.mocked(getConfigValue).mockReturnValue(
      schemaValueToWit(declaration.codec.toValue(undefined)),
    );
    expect(config.max_chars).toBeUndefined();
  });
});
