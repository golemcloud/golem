import { z } from 'zod';
import {
  defineAgent,
  method,
  s,
  awaitPromise,
  createPromise,
  Uuid,
  isRemoteCallError,
} from '@golemcloud/golem-ts-sdk';
import type { PromiseId } from 'golem:api/host@1.5.0';
import { getConfigValue } from 'golem:agent/host@2.0.0';
import { reveal } from 'golem:secrets/reveal@0.1.0';
import type { SecretError } from 'golem:secrets/types@0.1.0';
import {
  compileSchema,
  schemaGraphToWit,
  schemaValueFromWit,
} from '@golemcloud/golem-ts-sdk/schema';

// A `PromiseId` is a nested host record carrying bigints and a UUID; declare
// it as an explicit Standard Schema so it can be returned / accepted by a
// method.
const PromiseIdSchema = z.object({
  agentId: z.object({
    uuid: s.uuid(),
  }),
  fingerprint: s.uuid(),
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
    revealHeldSecretAcrossReplayGates: method({
      input: { beforeDelete: PromiseIdSchema, afterDelete: PromiseIdSchema },
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
      const promiseId = createPromise();
      return {
        ...promiseId,
        agentId: {
          ...promiseId.agentId,
          uuid: Uuid.from(promiseId.agentId.uuid),
        },
        fingerprint: Uuid.from(promiseId.fingerprint),
      };
    },
    async revealSecretThenAwaitReplayGate({ promiseId }) {
      const config = this.config;
      const secret = config.secret.get();
      await awaitPromise({
        ...promiseId,
        agentId: {
          ...promiseId.agentId,
          uuid: promiseId.agentId.uuid,
        },
      } as PromiseId);
      return secret;
    },
    async revealHeldSecretAcrossReplayGates({ beforeDelete, afterDelete }) {
      const codec = compileSchema(z.string());
      const tree = getConfigValue(
        ['secret'],
        schemaGraphToWit(compileSchema(s.secret(z.string())).graph),
      );
      const node = tree.valueNodes[tree.root];
      if (node?.tag !== 'secret-value') throw new Error('expected a secret handle');
      const handle = node.val;
      try {
        const before = codec.fromValue(
          schemaValueFromWit(reveal(handle, schemaGraphToWit(codec.graph))),
        );
        await awaitPromise(beforeDelete as PromiseId);
        let version: number[];
        try {
          reveal(handle, schemaGraphToWit(codec.graph));
          throw new Error('held handle must fail after deletion');
        } catch (error) {
          const failure = (
            error instanceof Error && 'payload' in error ? error.payload : error
          ) as SecretError;
          if (failure.tag !== 'version-not-found') throw error;
          version = Array.from(failure.val.bytes);
        }
        await awaitPromise(afterDelete as PromiseId);
        return JSON.stringify({ before, error: 'version-not-found', version });
      } finally {
        (handle as typeof handle & { [Symbol.dispose](): void })[Symbol.dispose]();
      }
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

// `RpcLocalConfigAgent` creates the keyed `LocalConfigAgent` with config overrides via
// `definition.client.create(id, { keyed: true, config })`: the non-secret override leaves
// present in `config` are encoded into the created agent's `agentConfig` list (see
// `agent_config/rpc.rs`). An existing agent keeps its configuration and is reused.
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
      // Override `foo`, and `nested.a` only when it is actually set (omitting an
      // undefined leaf leaves the callee's manifest value in place).
      const overrides: { foo: number; nested?: { a: boolean } } = { foo: config.foo };
      if (config.nested_a !== undefined) {
        overrides.nested = { a: config.nested_a };
      }
      let client;
      try {
        client = await LocalConfigAgent.client.create(
          { _name: this.name },
          { keyed: true, config: overrides },
        );
      } catch (error) {
        if (!isRemoteCallError(error) || error.cause.tag !== 'conflict') {
          throw error;
        }
        client = LocalConfigAgent.client.upsert({ _name: this.name });
      }
      return await client.echoLocalConfig();
    },
  },
});
