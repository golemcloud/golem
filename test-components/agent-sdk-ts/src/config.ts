import { z } from 'zod';
import {
  defineAgent,
  method,
  s,
  clientFor,
  awaitPromise,
  createPromise,
} from '@golemcloud/golem-ts-sdk';
import type { PromiseId } from 'golem:api/host@1.5.0';

// A `PromiseId` is a nested host record carrying bigints; declare it as an
// explicit Standard Schema so it can be returned / accepted by a method.
const PromiseIdSchema = z.object({
  agentId: z.object({
    componentId: z.object({
      uuid: z.object({ highBits: s.u64(), lowBits: s.u64() }),
    }),
    agentId: z.string(),
  }),
  oplogIdx: s.u64(),
});

export const ConfigAgent = defineAgent({
  name: 'ConfigAgent',
  id: { _name: z.string() },
  config: {
    foo: z.number(),
    bar: z.string(),
    secret: s.secret(z.string()),
    nested: z.object({
      nestedSecret: s.secret(z.number()),
      a: z.boolean(),
      b: z.array(z.number()),
    }),
    aliasedNested: z.object({
      c: z.number().optional(),
    }),
  },
  methods: {
    echoLocalConfig: method({ input: {}, returns: z.string() }),
  },
});

export const ConfigAgentImpl = ConfigAgent.implement({
  init: () => ({}),
  methods: {
    echoLocalConfig() {
      const config = this.config;
      return JSON.stringify({
        foo: config.foo,
        bar: config.bar,
        secret: config.secret.get(),
        nested: {
          nestedSecret: config.nested.nestedSecret.get(),
          a: config.nested.a,
          b: config.nested.b,
        },
        aliasedNested: {
          c: config.aliasedNested.c,
        },
      });
    },
  },
});

export const LocalConfigAgent = defineAgent({
  name: 'LocalConfigAgent',
  id: { _name: z.string() },
  config: {
    foo: z.number(),
    bar: z.string(),
    nested: z.object({
      a: z.boolean(),
      b: z.array(z.number()),
    }),
    aliasedNested: z.object({
      c: z.number().optional(),
    }),
  },
  methods: {
    echoLocalConfig: method({ input: {}, returns: z.string() }),
  },
});

export const LocalConfigAgentImpl = LocalConfigAgent.implement({
  init: () => ({}),
  methods: {
    echoLocalConfig() {
      const config = this.config;
      return JSON.stringify({
        foo: config.foo,
        bar: config.bar,
        nested: {
          a: config.nested.a,
          b: config.nested.b,
        },
        aliasedNested: {
          c: config.aliasedNested.c,
        },
      });
    },
  },
});

export const SharedConfigAgent = defineAgent({
  name: 'SharedConfigAgent',
  id: { _name: z.string() },
  config: {
    secret: s.secret(z.string()),
    complexSecret: s.secret(z.object({ foo: z.string(), bar: z.number() })),
  },
  methods: {
    echoLocalConfig: method({ input: {}, returns: z.string() }),
    createReplayGate: method({ input: {}, returns: PromiseIdSchema }),
    revealSecretThenAwaitReplayGate: method({
      input: { promiseId: PromiseIdSchema },
      returns: z.string(),
    }),
  },
});

export const SharedConfigAgentImpl = SharedConfigAgent.implement({
  init: () => ({}),
  methods: {
    echoLocalConfig() {
      const config = this.config;
      return JSON.stringify({
        secret: config.secret.get(),
        complexSecret: config.complexSecret.get(),
      });
    },
    createReplayGate() {
      return createPromise();
    },
    async revealSecretThenAwaitReplayGate({ promiseId }) {
      const config = this.config;
      const secret = config.secret.get();
      await awaitPromise(promiseId as unknown as PromiseId);
      return secret;
    },
  },
});

export const LocalCasingSharedConfigAgent = defineAgent({
  name: 'LocalCasingSharedConfigAgent',
  id: { _name: z.string() },
  config: {
    secretPath: s.secret(z.string()),
  },
  methods: {
    echoLocalConfig: method({ input: {}, returns: z.string() }),
  },
});

export const LocalCasingSharedConfigAgentImpl = LocalCasingSharedConfigAgent.implement({
  init: () => ({}),
  methods: {
    echoLocalConfig() {
      const config = this.config;
      return JSON.stringify({
        secretPath: config.secretPath.get(),
      });
    },
  },
});

export const OptionalGroupConfigAgent = defineAgent({
  name: 'OptionalGroupConfigAgent',
  id: { _name: z.string() },
  config: {
    required: z.string(),
    optionalGroup: z
      .object({
        a: z.number(),
        b: z.string().optional(),
      })
      .optional(),
  },
  methods: {
    echoLocalConfig: method({ input: {}, returns: z.string() }),
  },
});

export const OptionalGroupConfigAgentImpl = OptionalGroupConfigAgent.implement({
  init: () => ({}),
  methods: {
    echoLocalConfig() {
      const config = this.config;
      return JSON.stringify({
        required: config.required,
        optionalGroup: config.optionalGroup
          ? { a: config.optionalGroup.a, b: config.optionalGroup.b }
          : undefined,
      });
    },
  },
});

export const AllOptionalGroupConfigAgent = defineAgent({
  name: 'AllOptionalGroupConfigAgent',
  id: { _name: z.string() },
  config: {
    allOptionalGroup: z
      .object({
        x: z.number().optional(),
        y: z.string().optional(),
      })
      .optional(),
  },
  methods: {
    echoLocalConfig: method({ input: {}, returns: z.string() }),
  },
});

export const AllOptionalGroupConfigAgentImpl = AllOptionalGroupConfigAgent.implement({
  init: () => ({}),
  methods: {
    echoLocalConfig() {
      const config = this.config;
      return JSON.stringify({
        allOptionalGroup: config.allOptionalGroup ?? null,
      });
    },
  },
});

export const NestedRequiredGroupConfigAgent = defineAgent({
  name: 'NestedRequiredGroupConfigAgent',
  id: { _name: z.string() },
  config: {
    outer: z
      .object({
        required: z.string(),
        inner: z.object({
          a: z.number(),
        }),
      })
      .optional(),
  },
  methods: {
    echoLocalConfig: method({ input: {}, returns: z.string() }),
  },
});

export const NestedRequiredGroupConfigAgentImpl = NestedRequiredGroupConfigAgent.implement({
  init: () => ({}),
  methods: {
    echoLocalConfig() {
      const config = this.config;
      return JSON.stringify({
        outer: config.outer
          ? { required: config.outer.required, inner: { a: config.outer.inner.a } }
          : undefined,
      });
    },
  },
});

// `RpcLocalConfigAgent` invokes `LocalConfigAgent` with per-call config
// overrides via the config-on-RPC form `clientFor(def)(id, phantomId?, config)`:
// the non-secret override leaves present in `config` are encoded into the target
// `WasmRpc`'s `agentConfig` list (see `agent_config/rpc.rs`).
const localConfigClient = clientFor(LocalConfigAgent);

export const RpcLocalConfigAgent = defineAgent({
  name: 'RpcLocalConfigAgent',
  id: { name: z.string() },
  config: {
    foo: z.number(),
    nested_a: z.boolean().optional(),
  },
  methods: {
    echoLocalConfig: method({ input: {}, returns: z.string() }),
  },
});

export const RpcLocalConfigAgentImpl = RpcLocalConfigAgent.implement({
  init: ({ id }) => ({ name: id.name }),
  methods: {
    async echoLocalConfig() {
      const config = this.config;
      // Mirror the decorator agent's `getWithConfig(name, { foo, nested: { a } })`:
      // override `foo`, and `nested.a` only when it is actually set (omitting an
      // undefined leaf leaves the callee's manifest value in place).
      const overrides: Record<string, unknown> = { foo: config.foo };
      if (config.nested_a !== undefined) {
        overrides.nested = { a: config.nested_a };
      }
      const client = localConfigClient({ _name: this.name }, undefined, overrides);
      return await client.echoLocalConfig();
    },
  },
});
